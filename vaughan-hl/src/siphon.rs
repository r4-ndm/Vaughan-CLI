//! Automated Perp-to-Spot Profit Siphon (Liberty Bridge Intent Engine)
//!
//! # Architecture & Fund Boundary Note (Intent-Only):
//! Perpetual trading profits accrue as USDC on Hyperliquid L1 (or Arbitrum for Aster DEX).
//! The accumulation target token resides on PulseChain (Chain ID 369).
//!
//! This module does NOT bridge funds across chain boundaries and does NOT sign or
//! broadcast transactions directly. Instead, it evaluates rolling perpetual fills,
//! validates the zero-loss capital preservation gate, queries LibertySwap for a
//! cross-chain bridge quote (Arbitrum USDC -> PulseChain token), and produces a sized,
//! quoted [`SiphonIntent`].
//!
//! Cross-chain execution currently uses LibertySwap only. PulseChain DEX aggregator
//! helpers (SquirrelSwap & PulseSwap) in this module are diagnostic/reference utilities
//! and are not used for live multi-aggregator bridge routing.
//!
//! Actual execution (ERC-20 token approval and on-chain swap broadcast via the wallet's
//! local signer) is handed off to the caller (e.g., `vaughan-tui` or `WalletState`).

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

use crate::error::HlError;
use crate::market_data::LiveTradeFill;
use crate::strategy::{AutoReinvestConfig, ReinvestCadence};

/// Native token sentinel address for EVM / PulseChain swaps.
pub const NATIVE_ZERO_ADDR: &str = "0x0000000000000000000000000000000000000000";

/// Default meme coin BTC contract address on PulseChain mainnet (Chain ID 369).
/// Note: this is a PulseChain meme coin (`0xF7bf2A93...`), NOT native or wrapped Bitcoin.
pub const DEFAULT_BTC_MEME_ADDR: &str = "0xF7bf2A938f971D7e4811A1170C43d651d21A0F81";

/// PulseX token contract address on PulseChain.
pub const PLSX_CONTRACT_ADDR: &str = "0x8a810ea8B0Aeb6d003a091736154777c8bAc828d";

/// HEX contract address on PulseChain.
pub const HEX_CONTRACT_ADDR: &str = "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";

/// PulseX Incentive token (INC) contract address.
pub const INC_CONTRACT_ADDR: &str = "0x2fa878Ab3F87CC1C9737Fc071108F904c0B0C950";

/// DAI contract address on PulseChain.
pub const DAI_CONTRACT_ADDR: &str = "0xefD766cCb38EaF1dfd70166A9883412357596986";

/// Wrapped PLS (WPLS) contract address.
pub const WPLS_CONTRACT_ADDR: &str = "0xA1077a294dDE1B09bB078844df40758a5D0f9a27";

/// Canonical native USDC on Arbitrum One (Chain ID 42161, 6 decimals).
pub const ARBITRUM_USDC_ADDR: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";

/// Arbitrum One Chain ID.
pub const ARBITRUM_CHAIN_ID: u64 = 42161;

/// Official routers from Liberty docs + current unified router + live Arbitrum quote router.
/// Mirrors `vaughan_core::core::bridge::OFFICIAL_ROUTERS`.
pub const OFFICIAL_ROUTERS: &[&str] = &[
    // Docs table (legacy per-asset routers)
    "0xe7EE706a6708b691a232452c9cb267d186942F09", // PulseChain USDC
    "0x80C2C603d72ea17A0D85B670D4489eB3012035Cd", // PulseChain WETH
    "0x06291eeE038e94E8DEC2b3bfB6e030c0b5615506", // Ethereum USDC
    "0x12352B55e0b4305Dd83A349A5d7845bE9B5a2Eea", // Ethereum USDT
    "0xAA7a195D69327a894eeb969D3bCb89116FC78A14", // Ethereum DAI
    "0x60FDAf9198eFCD6fAF27D50E955e1A42905f2eeb", // Ethereum ETH
    "0x43f403972080406e3e6602793A5072DBc4389bAb", // BSC USDC
    "0xc438D51F296fF3e53d061293D2bC4Bb9fb2f7f19", // BSC USDT
    "0x4E839dA8DCd61df10976B926cbF9Ab7D06BfF072", // BSC USD1
    "0xefB11856C4bE75C276A5C9E286F8032D3E16Ced2", // Base USDC
    "0x05216280d45Bb8E8dcb863186E4762090bab7b6F", // Arbitrum USDC
    "0xcb2b2a70F29a8b7467fA930A09f9271D1eF0E5A9", // Polygon USDC
    // Live v3 unified router (Base↔Pulse USDC quotes, 2026-08)
    "0x78f63fe16C83728c16C0aE44b0c19D7dD105c215",
    // Live Arbitrum -> PulseChain unified router (2026-11)
    "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
];

/// Returns true when `router` is on the official Liberty router allowlist.
pub fn is_whitelisted_router(router: Address) -> bool {
    OFFICIAL_ROUTERS
        .iter()
        .filter_map(|s| s.parse::<Address>().ok())
        .any(|a| a == router)
}

/// Resolve a ticker symbol or raw 42-char hex string to an on-chain [`Address`].
pub fn resolve_target_token_address(token: &str) -> Result<Address, HlError> {
    let trimmed = token.trim();
    if trimmed.eq_ignore_ascii_case("PLS") || trimmed == NATIVE_ZERO_ADDR {
        return Ok(Address::ZERO);
    }
    if trimmed.eq_ignore_ascii_case("BTC")
        || trimmed.eq_ignore_ascii_case("BTC (meme)")
        || trimmed.eq_ignore_ascii_case(DEFAULT_BTC_MEME_ADDR)
    {
        return Address::from_str(DEFAULT_BTC_MEME_ADDR)
            .map_err(|e| HlError::InvalidStrategy(format!("invalid BTC meme address: {e}")));
    }
    if trimmed.eq_ignore_ascii_case("PLSX") || trimmed.eq_ignore_ascii_case(PLSX_CONTRACT_ADDR) {
        return Address::from_str(PLSX_CONTRACT_ADDR)
            .map_err(|e| HlError::InvalidStrategy(format!("invalid PLSX address: {e}")));
    }
    if trimmed.eq_ignore_ascii_case("HEX") || trimmed.eq_ignore_ascii_case(HEX_CONTRACT_ADDR) {
        return Address::from_str(HEX_CONTRACT_ADDR)
            .map_err(|e| HlError::InvalidStrategy(format!("invalid HEX address: {e}")));
    }
    if trimmed.eq_ignore_ascii_case("INC") || trimmed.eq_ignore_ascii_case(INC_CONTRACT_ADDR) {
        return Address::from_str(INC_CONTRACT_ADDR)
            .map_err(|e| HlError::InvalidStrategy(format!("invalid INC address: {e}")));
    }
    if trimmed.eq_ignore_ascii_case("DAI") || trimmed.eq_ignore_ascii_case(DAI_CONTRACT_ADDR) {
        return Address::from_str(DAI_CONTRACT_ADDR)
            .map_err(|e| HlError::InvalidStrategy(format!("invalid DAI address: {e}")));
    }
    if trimmed.eq_ignore_ascii_case("WPLS") || trimmed.eq_ignore_ascii_case(WPLS_CONTRACT_ADDR) {
        return Address::from_str(WPLS_CONTRACT_ADDR)
            .map_err(|e| HlError::InvalidStrategy(format!("invalid WPLS address: {e}")));
    }

    Address::from_str(trimmed).map_err(|e| {
        HlError::InvalidStrategy(format!(
            "cannot parse token contract address '{trimmed}': {e}"
        ))
    })
}

/// Return a concise display symbol for known addresses or format short hex `0x1234…5678`.
pub fn display_token_symbol(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.eq_ignore_ascii_case("PLS") || trimmed == NATIVE_ZERO_ADDR {
        return "PLS".to_string();
    }
    if trimmed.eq_ignore_ascii_case("BTC")
        || trimmed.eq_ignore_ascii_case("BTC (meme)")
        || trimmed.eq_ignore_ascii_case(DEFAULT_BTC_MEME_ADDR)
    {
        return "BTC (meme)".to_string();
    }
    if trimmed.eq_ignore_ascii_case("PLSX") || trimmed.eq_ignore_ascii_case(PLSX_CONTRACT_ADDR) {
        return "PLSX".to_string();
    }
    if trimmed.eq_ignore_ascii_case("HEX") || trimmed.eq_ignore_ascii_case(HEX_CONTRACT_ADDR) {
        return "HEX".to_string();
    }
    if trimmed.eq_ignore_ascii_case("INC") || trimmed.eq_ignore_ascii_case(INC_CONTRACT_ADDR) {
        return "INC".to_string();
    }
    if trimmed.eq_ignore_ascii_case("DAI") || trimmed.eq_ignore_ascii_case(DAI_CONTRACT_ADDR) {
        return "DAI".to_string();
    }
    if trimmed.eq_ignore_ascii_case("WPLS") || trimmed.eq_ignore_ascii_case(WPLS_CONTRACT_ADDR) {
        return "WPLS".to_string();
    }
    if trimmed.len() > 10 && trimmed.starts_with("0x") {
        return format!("{}…{}", &trimmed[0..6], &trimmed[trimmed.len() - 4..]);
    }
    trimmed.to_string()
}

/// Normalized quote response from any individual aggregator venue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatorQuoteResult {
    pub venue: String,
    pub amount_in: String,
    pub amount_out: String,
    pub amount_out_u256: U256,
    pub route: String,
    pub gas_hint: String,
}

/// Query SquirrelSwap's live public brain (`https://api.squirrelswap.pro/quote`).
pub async fn query_squirrelswap(
    client: &reqwest::Client,
    token_in: &str,
    token_out: &str,
    amount_in_wei: &str,
) -> Result<AggregatorQuoteResult, HlError> {
    let url = format!(
        "https://api.squirrelswap.pro/quote?tokenIn={token_in}&tokenOut={token_out}&amountIn={amount_in_wei}&compact=1"
    );
    let resp = client
        .get(&url)
        .header("X-SS-Client", "vaughan-hl/0.1")
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| HlError::MarketData(format!("squirrelswap request failed: {e}")))?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|e| {
        HlError::MarketData(format!("squirrelswap invalid JSON (HTTP {status}): {e}"))
    })?;

    if !body
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let msg = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("no route");
        return Err(HlError::MarketData(format!(
            "squirrelswap quote rejected: {msg}"
        )));
    }

    let amt_str = body
        .get("amountOut")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HlError::MarketData("squirrelswap missing amountOut".into()))?;

    let route = body
        .get("route")
        .and_then(|v| v.as_str())
        .unwrap_or("SquirrelSwap")
        .to_string();

    let gas = body
        .get("gasPLS")
        .and_then(|v| v.as_str())
        .unwrap_or("140")
        .to_string();

    let u256 = U256::from_str(amt_str).unwrap_or(U256::ZERO);

    Ok(AggregatorQuoteResult {
        venue: "SquirrelSwap".to_string(),
        amount_in: amount_in_wei.to_string(),
        amount_out: amt_str.to_string(),
        amount_out_u256: u256,
        route,
        gas_hint: format!("{gas} PLS"),
    })
}

/// Query PulseSwap's live advanced quote engine (`https://quotes.pulseswap.io/api/v2/quotes/advanced`).
pub async fn query_pulseswap(
    client: &reqwest::Client,
    token_in: &str,
    token_out: &str,
    amount_in_wei: &str,
    slippage: f64,
) -> Result<AggregatorQuoteResult, HlError> {
    let url = "https://quotes.pulseswap.io/api/v2/quotes/advanced";
    let body = serde_json::json!({
        "chainId": 369,
        "platform": "mixed",
        "fromToken": token_in,
        "toToken": token_out,
        "amountIn": amount_in_wei,
        "slippage": slippage,
        "extra": {
            "tokenInPrice": 1.0,
            "tokenOutPrice": 1.0,
            "gasPrice": "1000000000",
            "gasTokenPrice": 1.0
        }
    });

    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "vaughan-hl/0.1")
        .json(&body)
        .send()
        .await
        .map_err(|e| HlError::MarketData(format!("pulseswap request failed: {e}")))?;

    let status = resp.status();
    let val: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| HlError::MarketData(format!("pulseswap invalid JSON (HTTP {status}): {e}")))?;

    let data = val.get("data").unwrap_or(&val);
    let amt_str = data
        .get("amountOut")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HlError::MarketData("pulseswap missing amountOut".into()))?;

    let gas = data
        .get("gasEstimate")
        .map(|v| v.to_string())
        .unwrap_or_else(|| "175000".to_string());

    let u256 = U256::from_str(amt_str).unwrap_or(U256::ZERO);

    Ok(AggregatorQuoteResult {
        venue: "PulseSwap".to_string(),
        amount_in: amount_in_wei.to_string(),
        amount_out: amt_str.to_string(),
        amount_out_u256: u256,
        route: "PulseSwap Native".to_string(),
        gas_hint: format!("{gas} gas"),
    })
}

/// Meta-Aggregator: Query all live aggregators in parallel and select the optimal route
/// that delivers the highest net token output.
///
/// NOTE: This is a CLI diagnostic helper (`vaughan-hl siphon`). Not used in the live cross-chain runtime bridge path.
pub async fn quote_best_aggregator(
    token_in: &str,
    token_out: &str,
    amount_in_wei: &str,
    slippage: f64,
) -> Result<AggregatorQuoteResult, HlError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| HlError::MarketData(format!("cannot build HTTP client: {e}")))?;

    let (res_sq, res_ps) = tokio::join!(
        query_squirrelswap(&client, token_in, token_out, amount_in_wei),
        query_pulseswap(&client, token_in, token_out, amount_in_wei, slippage)
    );

    let mut outcomes: Vec<AggregatorQuoteResult> = Vec::new();
    if let Ok(q) = res_sq {
        outcomes.push(q);
    }
    if let Ok(q) = res_ps {
        outcomes.push(q);
    }

    if outcomes.is_empty() {
        return Err(HlError::MarketData(
            "all PulseChain aggregators returned empty or unroutable quotes".into(),
        ));
    }

    // Rank by amount_out_u256 descending: winner is index 0
    outcomes.sort_by_key(|b| std::cmp::Reverse(b.amount_out_u256));
    Ok(outcomes.remove(0))
}

/// Dynamically derive the live PLS-per-USD price by quoting 1 DAI (`0xefD7...`) to WPLS (`0xA107...`).
///
/// NOTE: This is a CLI diagnostic helper (`vaughan-hl siphon`). Not used in the live cross-chain runtime bridge path.
pub async fn derive_pls_per_usd(client: &reqwest::Client) -> Result<f64, HlError> {
    let one_dai_wei = "1000000000000000000"; // 1 DAI (18 decimals)
    let (res_sq, res_ps) = tokio::join!(
        query_squirrelswap(client, DAI_CONTRACT_ADDR, WPLS_CONTRACT_ADDR, one_dai_wei),
        query_pulseswap(
            client,
            DAI_CONTRACT_ADDR,
            WPLS_CONTRACT_ADDR,
            one_dai_wei,
            1.0
        )
    );

    let mut outcomes: Vec<AggregatorQuoteResult> = Vec::new();
    if let Ok(q) = res_sq {
        outcomes.push(q);
    }
    if let Ok(q) = res_ps {
        outcomes.push(q);
    }

    if outcomes.is_empty() {
        return Err(HlError::MarketData("no PLS price available".into()));
    }

    outcomes.sort_by_key(|b| std::cmp::Reverse(b.amount_out_u256));
    let best = outcomes.remove(0);
    if best.amount_out_u256 == U256::ZERO {
        return Err(HlError::MarketData("no PLS price available".into()));
    }

    let parsed = best
        .amount_out
        .parse::<f64>()
        .map_err(|e| HlError::MarketData(format!("invalid rate string: {e}")))?;
    let rate = parsed / 1e18;
    if rate <= 0.0 {
        return Err(HlError::MarketData("no PLS price available".into()));
    }
    Ok(rate)
}

/// Compute net realized profit (`closed_pnl - fee`) for fills closed since `epoch_start_ms`.
pub fn compute_rolling_net_pnl(fills: &[LiveTradeFill], epoch_start_ms: u64) -> f64 {
    fills
        .iter()
        .filter(|f| f.time_ms >= epoch_start_ms && f.dir.starts_with("Close"))
        .map(|f| f.closed_pnl - f.fee)
        .sum()
}

/// Check if the profit siphon settlement epoch has elapsed.
pub fn is_epoch_due(last_epoch_ms: u64, now_ms: u64, cadence: ReinvestCadence) -> bool {
    let elapsed = now_ms.saturating_sub(last_epoch_ms);
    match cadence {
        ReinvestCadence::Weekly => elapsed >= 7 * 86_400_000,
        ReinvestCadence::Daily => elapsed >= 86_400_000,
        ReinvestCadence::PerTrade => elapsed > 0,
    }
}

/// Status of a profit siphon evaluation epoch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SiphonStatus {
    /// Quote obtained; calldata ready, nothing signed or sent.
    Quoted,
    /// A real transaction was broadcast; tx_hash is the on-chain hash.
    Broadcast { tx_hash: String },
    /// Gate refused this epoch (drawdown, below threshold, disabled, no route).
    Skipped { reason: String },
}

/// Execution hand-off ready for the caller (e.g. `vaughan-tui` or `WalletState`)
/// to sign and broadcast the spot swap on PulseChain (Chain ID 369).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiphonIntent {
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub slippage_bps: u32,
    pub chain_id: u64,
    pub winning_venue: String,
    pub route_description: String,
    pub amount_out_tokens: String,
}

/// Execution record of one completed, quoted, or skipped profit siphon epoch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiphonReceipt {
    pub timestamp_ms: u64,
    pub net_profit_usd: f64,
    pub profit_pct: f64,
    pub siphon_usd: f64,
    pub target_token: String,
    pub target_symbol: String,
    pub winning_aggregator: Option<String>,
    pub route_description: Option<String>,
    pub amount_out_tokens: Option<String>,
    pub status: SiphonStatus,
    pub intent: Option<SiphonIntent>,
    pub dry_run: bool,
}

static LAST_SIPHON_RECEIPT: Mutex<Option<SiphonReceipt>> = Mutex::new(None);

/// Record the most recent siphon receipt into process-wide in-memory cache for TUI telemetry.
pub fn set_last_siphon_receipt(receipt: SiphonReceipt) {
    if let Ok(mut g) = LAST_SIPHON_RECEIPT.lock() {
        *g = Some(receipt);
    }
}

/// Retrieve the most recent siphon receipt from in-memory cache without file IO.
pub fn last_siphon_receipt() -> Option<SiphonReceipt> {
    LAST_SIPHON_RECEIPT.lock().ok().and_then(|g| g.clone())
}

/// Clear the cached siphon receipt.
pub fn clear_last_siphon_receipt() {
    if let Ok(mut g) = LAST_SIPHON_RECEIPT.lock() {
        *g = None;
    }
}

/// Normalized cross-chain bridge quote from LibertySwap (Arbitrum USDC -> PulseChain token).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibertyBridgeQuote {
    pub router: Address,   // quote "to"
    pub usdc: Address,     // srcToken.address (Arbitrum USDC)
    pub target: Address,   // destToken.address
    pub amount_usdc: U256, // srcAmount (6 decimals)
    pub dest_amount: U256, // destAmount
    pub approval_spender: Address,
    pub approval_calldata: Vec<u8>,
    pub bridge_calldata: Vec<u8>,
    pub bridge_value: U256,
    pub fee_percent: f64,
}

/// Pending-execution batch saved to `data/siphon_pending.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiphonPending {
    pub created_ms: u64,
    pub recipient: Address,
    pub chain_id: u64,
    pub amount_usdc: U256,
    pub target_symbol: String,
    pub target_token: String,
    pub router: Address,
    pub approval_spender: Address,
    pub approval_calldata: String,
    pub bridge_calldata: String,
    pub bridge_value: U256,
    pub dest_amount: U256,
    pub fee_percent: f64,
    pub net_profit_usd: f64,
    pub profit_pct: f64,
}

/// Wallet data directory for Vaughan CLI (`$HOME/.local/share/vaughan-cli/`).
pub fn wallet_data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .map(|d| d.join("vaughan-cli"))
        .unwrap_or_else(|| {
            if let Ok(home) = std::env::var("HOME") {
                std::path::PathBuf::from(home).join(".local/share/vaughan-cli")
            } else {
                std::path::PathBuf::from(".local/share/vaughan-cli")
            }
        })
}

/// Path to the pending cross-chain execution batch file.
pub fn pending_exec_path() -> std::path::PathBuf {
    wallet_data_dir().join("siphon_pending.json")
}

/// Path to the persisted profit siphon rolling window state file.
pub fn siphon_state_path() -> std::path::PathBuf {
    wallet_data_dir().join("siphon_state.json")
}

/// Persisted rolling window state across runner restarts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SiphonWindowState {
    pub epoch_start_ms: u64,
    pub last_siphon_ms: u64,
}

/// Load the persisted rolling window state from disk if it exists.
pub fn load_siphon_state() -> Option<SiphonWindowState> {
    load_siphon_state_from_path(&siphon_state_path())
}

/// Load the persisted rolling window state from an explicit file path.
pub fn load_siphon_state_from_path(path: &std::path::Path) -> Option<SiphonWindowState> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save the rolling window state to disk atomically.
pub fn save_siphon_state(state: &SiphonWindowState) {
    save_siphon_state_to_path(&siphon_state_path(), state);
}

/// Save the rolling window state to an explicit file path.
pub fn save_siphon_state_to_path(path: &std::path::Path, state: &SiphonWindowState) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(path, json);
    }
}

/// Write `SiphonPending` to `<wallet_data_dir>/siphon_pending.json` (overwrites atomically).
pub fn write_pending_execution(pending: &SiphonPending) {
    if let Ok(json) = serde_json::to_string_pretty(pending) {
        let p = pending_exec_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&p, json);
    }
}

/// Parse and validate a LibertySwap v3 quote JSON response without network access.
pub fn parse_liberty_quote_json(
    body: &serde_json::Value,
    req_amount: U256,
    expected_target: Address,
    expected_recipient: Address,
) -> Result<LibertyBridgeQuote, HlError> {
    let to_str = body
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HlError::MarketData("libertyswap quote missing 'to'".into()))?;
    let router = Address::from_str(to_str)
        .map_err(|e| HlError::MarketData(format!("invalid router address: {e}")))?;

    if !is_whitelisted_router(router) {
        return Err(HlError::InvalidStrategy(format!(
            "libertyswap router {router:#x} not on allowlist — refusing quote"
        )));
    }

    let dest_amount_val = body.get("destAmount").and_then(|v| {
        if let Some(s) = v.as_str() {
            U256::from_str(s).ok()
        } else {
            v.as_u64().map(U256::from)
        }
    });

    let dest_amount = match dest_amount_val {
        Some(d) if !d.is_zero() => d,
        _ => {
            return Err(HlError::MarketData(
                "libertyswap destAmount zero or absent".into(),
            ))
        }
    };

    let usdc = body
        .get("srcToken")
        .and_then(|t| t.get("address"))
        .and_then(|a| a.as_str())
        .and_then(|s| Address::from_str(s).ok())
        .ok_or_else(|| {
            HlError::MarketData("libertyswap quote missing or invalid 'srcToken.address'".into())
        })?;

    let target = body
        .get("destToken")
        .and_then(|t| t.get("address"))
        .and_then(|a| a.as_str())
        .and_then(|s| Address::from_str(s).ok())
        .ok_or_else(|| {
            HlError::MarketData("libertyswap quote missing or invalid 'destToken.address'".into())
        })?;

    if target != expected_target {
        return Err(HlError::MarketData(format!(
            "libertyswap quote destToken {target:#x} does not match expected target {expected_target:#x}"
        )));
    }

    let amount_usdc = body
        .get("srcAmount")
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                U256::from_str(s).ok()
            } else {
                v.as_u64().map(U256::from)
            }
        })
        .ok_or_else(|| {
            HlError::MarketData("libertyswap quote missing or invalid 'srcAmount'".into())
        })?;

    if amount_usdc != req_amount {
        return Err(HlError::MarketData(format!(
            "libertyswap quote srcAmount {amount_usdc} does not match requested amount {req_amount}"
        )));
    }

    let approval_obj = body
        .get("approval")
        .ok_or_else(|| HlError::MarketData("libertyswap quote missing 'approval'".into()))?;
    let approval_spender = approval_obj
        .get("spender")
        .and_then(|s| s.as_str())
        .and_then(|s| Address::from_str(s).ok())
        .ok_or_else(|| {
            HlError::MarketData("libertyswap quote missing or invalid 'approval.spender'".into())
        })?;

    if !is_whitelisted_router(approval_spender) {
        return Err(HlError::InvalidStrategy(format!(
            "libertyswap approval spender {approval_spender:#x} not on allowlist — refusing quote"
        )));
    }

    let approval_calldata_str = approval_obj
        .get("calldata")
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            HlError::MarketData("libertyswap quote missing 'approval.calldata'".into())
        })?;

    let approval_calldata = hex::decode(approval_calldata_str.trim_start_matches("0x"))
        .map_err(|e| HlError::MarketData(format!("invalid approval calldata hex: {e}")))?;

    let bridge_calldata_str = body
        .get("methodParameters")
        .and_then(|m| m.get("calldata"))
        .or_else(|| body.get("calldata"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            HlError::MarketData("missing bridge calldata in libertyswap quote".into())
        })?;

    let bridge_calldata = hex::decode(bridge_calldata_str.trim_start_matches("0x"))
        .map_err(|e| HlError::MarketData(format!("invalid bridge calldata hex: {e}")))?;

    // Recipient binding check: require that the requested recipient address
    // (ABI-encoded as a 32-byte word, left-padded with 12 zero bytes) appears in bridge_calldata.
    // NOTE: This is a 32-byte presence check rather than a full ABI decode because LibertySwap
    // wraps quotes across various router versions with variable-length dynamic parameters and
    // nested bridge payloads whose internal struct schemas change upstream. A 32-byte presence
    // check strictly ensures the requested recipient is bound in the calldata without risking
    // breakage from upstream router ABI structural adjustments.
    if expected_recipient == Address::ZERO {
        return Err(HlError::InvalidStrategy(
            "refusing quote: requested recipient address cannot be zero".into(),
        ));
    }
    let mut recipient_word = [0u8; 32];
    recipient_word[12..32].copy_from_slice(expected_recipient.as_slice());
    if !bridge_calldata.windows(32).any(|w| w == recipient_word) {
        return Err(HlError::MarketData(format!(
            "bridge calldata does not contain expected recipient {expected_recipient:#x}"
        )));
    }

    let bridge_value = body
        .get("methodParameters")
        .and_then(|m| m.get("value"))
        .or_else(|| body.get("value"))
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                U256::from_str(s).ok()
            } else {
                v.as_u64().map(U256::from)
            }
        })
        .unwrap_or(U256::ZERO);

    let fee_percent = body
        .get("fee")
        .and_then(|f| f.get("protocol"))
        .and_then(|p| p.get("percentage"))
        .and_then(|v| v.as_f64())
        .or_else(|| {
            body.get("fee")
                .and_then(|f| f.get("percentage"))
                .and_then(|v| v.as_f64())
        })
        .unwrap_or(0.30);

    Ok(LibertyBridgeQuote {
        router,
        usdc,
        target,
        amount_usdc,
        dest_amount,
        approval_spender,
        approval_calldata,
        bridge_calldata,
        bridge_value,
        fee_percent,
    })
}

/// Fetch a cross-chain quote directly from LibertySwap (Arbitrum USDC -> PulseChain default target).
pub async fn quote_liberty_bridge(
    amount_usdc: U256,
    recipient: Address,
) -> Result<LibertyBridgeQuote, HlError> {
    quote_liberty_bridge_for_token(amount_usdc, recipient, DEFAULT_BTC_MEME_ADDR).await
}

/// Fetch a cross-chain quote from LibertySwap for any specified PulseChain target token contract.
pub async fn quote_liberty_bridge_for_token(
    amount_usdc: U256,
    recipient: Address,
    target_token: &str,
) -> Result<LibertyBridgeQuote, HlError> {
    let resolved_target = resolve_target_token_address(target_token)?;
    let target_str = format!("{resolved_target:#x}");
    let url = format!(
        "https://apis.libertyswap.finance/v3/swap/quote?srcToken=USDC&dstToken={target_str}&amount={amount_usdc}&srcChain=42161&dstChain=369&recipient={recipient:#x}"
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .map_err(|e| HlError::MarketData(format!("cannot build HTTP client: {e}")))?;

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .header("User-Agent", "vaughan-hl/0.1")
        .send()
        .await
        .map_err(|e| HlError::MarketData(format!("libertyswap request failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(HlError::MarketData(format!("libertyswap HTTP {status}")));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| HlError::MarketData(format!("libertyswap JSON parse failed: {e}")))?;

    parse_liberty_quote_json(&body, amount_usdc, resolved_target, recipient)
}

/// In-memory manager tracking the rolling profit siphon state for the active runner.
pub struct SiphonManager {
    pub config: AutoReinvestConfig,
    pub recipient: Address,
    pub epoch_start_ms: Arc<AtomicU64>,
    pub last_siphon_ms: Arc<AtomicU64>,
    pub history: Vec<SiphonReceipt>,
}

impl SiphonManager {
    pub fn new(config: AutoReinvestConfig, now_ms: u64) -> Self {
        Self::new_with_recipient(config, now_ms, Address::ZERO)
    }

    pub fn new_with_recipient(config: AutoReinvestConfig, now_ms: u64, recipient: Address) -> Self {
        let (epoch_start, last_siphon) = if let Some(state) = load_siphon_state() {
            (state.epoch_start_ms, state.last_siphon_ms)
        } else {
            (now_ms, 0)
        };
        Self::new_with_state(config, epoch_start, last_siphon, recipient)
    }

    pub fn new_with_state(
        config: AutoReinvestConfig,
        epoch_start_ms: u64,
        last_siphon_ms: u64,
        recipient: Address,
    ) -> Self {
        Self {
            config,
            recipient,
            epoch_start_ms: Arc::new(AtomicU64::new(epoch_start_ms)),
            last_siphon_ms: Arc::new(AtomicU64::new(last_siphon_ms)),
            history: Vec::new(),
        }
    }

    /// Evaluates whether a siphon batch is due and produces the sized, quoted [`SiphonIntent`].
    pub async fn evaluate_and_siphon(
        &mut self,
        fills: &[LiveTradeFill],
        now_ms: u64,
    ) -> Result<Option<SiphonReceipt>, HlError> {
        if !self.config.enabled {
            let receipt = SiphonReceipt {
                timestamp_ms: now_ms,
                net_profit_usd: 0.0,
                profit_pct: self.config.profit_pct,
                siphon_usd: 0.0,
                target_token: self.config.target_token.clone(),
                target_symbol: self.config.target_token_symbol.clone(),
                winning_aggregator: None,
                route_description: None,
                amount_out_tokens: None,
                status: SiphonStatus::Skipped {
                    reason: "profit siphon disabled in strategy config".to_string(),
                },
                intent: None,
                dry_run: self.config.dry_run,
            };
            set_last_siphon_receipt(receipt.clone());
            self.history.push(receipt.clone());
            return Ok(Some(receipt));
        }

        let epoch_start = self.epoch_start_ms.load(Ordering::Relaxed);
        if !is_epoch_due(epoch_start, now_ms, self.config.cadence) {
            return Ok(None);
        }

        let net_pnl = compute_rolling_net_pnl(fills, epoch_start);

        // Zero-Loss Capital Protection Gate: skip if flat or below threshold.
        // NOTE: Do NOT reset epoch_start_ms on drawdown or below-threshold skips —
        // only advance after a real siphon, otherwise previous losses are erased from the window.
        if net_pnl <= 0.0 || net_pnl < self.config.min_profit_threshold_usd {
            let receipt = SiphonReceipt {
                timestamp_ms: now_ms,
                net_profit_usd: net_pnl,
                profit_pct: self.config.profit_pct,
                siphon_usd: 0.0,
                target_token: self.config.target_token.clone(),
                target_symbol: self.config.target_token_symbol.clone(),
                winning_aggregator: None,
                route_description: None,
                amount_out_tokens: None,
                status: SiphonStatus::Skipped {
                    reason: if net_pnl <= 0.0 {
                        "drawdown: net PnL <= 0".to_string()
                    } else {
                        format!(
                            "profit ${net_pnl:.2} below minimum threshold ${:.2}",
                            self.config.min_profit_threshold_usd
                        )
                    },
                },
                intent: None,
                dry_run: self.config.dry_run,
            };
            set_last_siphon_receipt(receipt.clone());
            self.history.push(receipt.clone());
            return Ok(Some(receipt));
        }

        let siphon_usd = net_pnl * (self.config.profit_pct / 100.0);
        let amount_usdc_units = (siphon_usd * 1e6) as u64;
        if amount_usdc_units == 0 {
            let receipt = SiphonReceipt {
                timestamp_ms: now_ms,
                net_profit_usd: net_pnl,
                profit_pct: self.config.profit_pct,
                siphon_usd: 0.0,
                target_token: self.config.target_token.clone(),
                target_symbol: self.config.target_token_symbol.clone(),
                winning_aggregator: None,
                route_description: None,
                amount_out_tokens: None,
                status: SiphonStatus::Skipped {
                    reason: "zero USDC amount after rounding".to_string(),
                },
                intent: None,
                dry_run: self.config.dry_run,
            };
            set_last_siphon_receipt(receipt.clone());
            self.history.push(receipt.clone());
            return Ok(Some(receipt));
        }

        let amount_usdc = U256::from(amount_usdc_units);
        let target_addr = resolve_target_token_address(&self.config.target_token)?;
        let target_addr_str = format!("{:#x}", target_addr);
        // Never guess where the funds land: a zero recipient means the runner did not
        // bind one, so the batch is refused rather than sent to a hardcoded address.
        let recipient = self.recipient;
        if recipient == Address::ZERO {
            let receipt = SiphonReceipt {
                timestamp_ms: now_ms,
                net_profit_usd: net_pnl,
                profit_pct: self.config.profit_pct,
                siphon_usd,
                target_token: target_addr_str.clone(),
                target_symbol: self.config.target_token_symbol.clone(),
                winning_aggregator: None,
                route_description: None,
                amount_out_tokens: None,
                status: SiphonStatus::Skipped {
                    reason: "no recipient address bound to this siphon".to_string(),
                },
                intent: None,
                dry_run: self.config.dry_run,
            };
            set_last_siphon_receipt(receipt.clone());
            self.history.push(receipt.clone());
            return Ok(Some(receipt));
        }

        // Quote LibertySwap cross-chain bridge (Arbitrum USDC -> PulseChain target token)
        let quote =
            match quote_liberty_bridge_for_token(amount_usdc, recipient, &self.config.target_token)
                .await
            {
                Ok(q) => q,
                Err(e) => {
                    let receipt = SiphonReceipt {
                        timestamp_ms: now_ms,
                        net_profit_usd: net_pnl,
                        profit_pct: self.config.profit_pct,
                        siphon_usd,
                        target_token: target_addr_str,
                        target_symbol: self.config.target_token_symbol.clone(),
                        winning_aggregator: None,
                        route_description: None,
                        amount_out_tokens: None,
                        status: SiphonStatus::Skipped {
                            reason: format!("liberty quote failed: {e}"),
                        },
                        intent: None,
                        dry_run: self.config.dry_run,
                    };
                    set_last_siphon_receipt(receipt.clone());
                    self.history.push(receipt.clone());
                    return Ok(Some(receipt));
                }
            };

        let usdc_addr = Address::from_str(ARBITRUM_USDC_ADDR).unwrap_or(quote.usdc);
        let intent = SiphonIntent {
            token_in: usdc_addr,
            token_out: target_addr,
            amount_in: amount_usdc,
            slippage_bps: self.config.max_slippage_bps,
            chain_id: 42161,
            winning_venue: "LibertySwap".to_string(),
            route_description: format!(
                "Arbitrum USDC -> PulseChain {} via Liberty",
                self.config.target_token_symbol
            ),
            amount_out_tokens: quote.dest_amount.to_string(),
        };

        let receipt = SiphonReceipt {
            timestamp_ms: now_ms,
            net_profit_usd: net_pnl,
            profit_pct: self.config.profit_pct,
            siphon_usd,
            target_token: target_addr_str.clone(),
            target_symbol: self.config.target_token_symbol.clone(),
            winning_aggregator: Some("LibertySwap".to_string()),
            route_description: Some(format!(
                "LibertySwap Bridge (fee {:.2}%)",
                quote.fee_percent
            )),
            amount_out_tokens: Some(quote.dest_amount.to_string()),
            status: SiphonStatus::Quoted,
            intent: Some(intent),
            dry_run: self.config.dry_run,
        };

        // Write siphon_pending.json under wallet_data_dir()
        let pending = SiphonPending {
            created_ms: now_ms,
            recipient,
            chain_id: 42161,
            amount_usdc,
            target_symbol: self.config.target_token_symbol.clone(),
            target_token: target_addr_str,
            router: quote.router,
            approval_spender: quote.approval_spender,
            approval_calldata: format!("0x{}", hex::encode(&quote.approval_calldata)),
            bridge_calldata: format!("0x{}", hex::encode(&quote.bridge_calldata)),
            bridge_value: quote.bridge_value,
            dest_amount: quote.dest_amount,
            fee_percent: quote.fee_percent,
            net_profit_usd: net_pnl,
            profit_pct: self.config.profit_pct,
        };
        write_pending_execution(&pending);

        self.epoch_start_ms.store(now_ms, Ordering::Relaxed);
        self.last_siphon_ms.store(now_ms, Ordering::Relaxed);
        save_siphon_state(&SiphonWindowState {
            epoch_start_ms: now_ms,
            last_siphon_ms: now_ms,
        });
        set_last_siphon_receipt(receipt.clone());
        self.history.push(receipt.clone());

        Ok(Some(receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_address_resolution() {
        assert_eq!(resolve_target_token_address("PLS").unwrap(), Address::ZERO);
        assert_eq!(
            resolve_target_token_address("BTC").unwrap(),
            Address::from_str("0xF7bf2A938f971D7e4811A1170C43d651d21A0F81").unwrap()
        );
        assert_eq!(
            resolve_target_token_address("PLSX").unwrap(),
            Address::from_str("0x8a810ea8B0Aeb6d003a091736154777c8bAc828d").unwrap()
        );
    }

    #[test]
    fn test_zero_loss_gate_blocks_negative_pnl() {
        let fills = vec![LiveTradeFill {
            oid: 1,
            coin: "BTC".into(),
            dir: "Close Long".into(),
            px: 70000.0,
            sz: 0.1,
            closed_pnl: -50.0,
            fee: 1.0,
            time_ms: 1000,
        }];
        let net = compute_rolling_net_pnl(&fills, 0);
        assert!(net < 0.0);
    }

    #[test]
    fn test_rolling_pnl_positive_accumulation() {
        let fills = vec![
            LiveTradeFill {
                oid: 1,
                coin: "BTC".into(),
                dir: "Close Long".into(),
                px: 70000.0,
                sz: 0.1,
                closed_pnl: 100.0,
                fee: 2.0,
                time_ms: 1000,
            },
            LiveTradeFill {
                oid: 2,
                coin: "ETH".into(),
                dir: "Close Short".into(),
                px: 3000.0,
                sz: 1.0,
                closed_pnl: 50.0,
                fee: 1.0,
                time_ms: 2000,
            },
        ];
        let net = compute_rolling_net_pnl(&fills, 0);
        assert_eq!(net, 147.0);
    }

    #[tokio::test]
    async fn test_skipped_when_net_pnl_below_threshold() {
        let config = AutoReinvestConfig {
            enabled: true,
            min_profit_threshold_usd: 10.0,
            ..Default::default()
        };
        let mut mgr = SiphonManager::new(config, 0);

        let fills = vec![LiveTradeFill {
            oid: 1,
            coin: "BTC".into(),
            dir: "Close Long".into(),
            px: 70000.0,
            sz: 0.1,
            closed_pnl: 5.0,
            fee: 0.5,
            time_ms: 1000,
        }];

        // Fire after 8 days (epoch due)
        let res = mgr
            .evaluate_and_siphon(&fills, 8 * 86_400_000)
            .await
            .unwrap();
        assert!(res.is_some());
        let receipt = res.unwrap();
        match receipt.status {
            SiphonStatus::Skipped { reason } => {
                assert!(reason.contains("below minimum threshold"));
            }
            other => panic!("expected Skipped status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_skipped_when_net_pnl_below_new_25_threshold() {
        let config = AutoReinvestConfig {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(config.min_profit_threshold_usd, 25.0);
        let mut mgr = SiphonManager::new(config, 0);

        let fills = vec![LiveTradeFill {
            oid: 1,
            coin: "BTC".into(),
            dir: "Close Long".into(),
            px: 70000.0,
            sz: 0.1,
            closed_pnl: 24.5,
            fee: 0.5,
            time_ms: 1000,
        }]; // Net PnL is 24.0 < 25.0

        let res = mgr
            .evaluate_and_siphon(&fills, 8 * 86_400_000)
            .await
            .unwrap();
        assert!(res.is_some());
        let receipt = res.unwrap();
        match receipt.status {
            SiphonStatus::Skipped { reason } => {
                assert!(reason.contains("below minimum threshold"));
                assert!(reason.contains("24.00"));
                assert!(reason.contains("25.00"));
            }
            other => panic!("expected Skipped status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_evaluate_and_siphon_returns_skipped_when_disabled() {
        let config = AutoReinvestConfig {
            enabled: false,
            ..Default::default()
        };
        let mut mgr = SiphonManager::new(config, 0);
        let fills = vec![LiveTradeFill {
            oid: 1,
            coin: "BTC".into(),
            dir: "Close Long".into(),
            px: 70000.0,
            sz: 0.1,
            closed_pnl: 100.0,
            fee: 1.0,
            time_ms: 1000,
        }];

        let res = mgr
            .evaluate_and_siphon(&fills, 8 * 86_400_000)
            .await
            .unwrap();
        assert!(
            res.is_some(),
            "must return Some(receipt) rather than silence"
        );
        let receipt = res.unwrap();
        match receipt.status {
            SiphonStatus::Skipped { reason } => {
                assert!(
                    reason.contains("disabled in strategy config"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected Skipped status, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_liberty_quote_json_strict_field_validation() {
        let req_amount = U256::from(1000);
        let target = Address::from_str(DEFAULT_BTC_MEME_ADDR).unwrap();
        let recipient = Address::from_str("0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63").unwrap();

        // 1. Missing srcToken
        let json_no_src = serde_json::json!({
            "to": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "destAmount": "5000",
            "destToken": { "address": DEFAULT_BTC_MEME_ADDR },
            "srcAmount": "1000",
            "approval": {
                "spender": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
                "calldata": "0x095ea7b3"
            },
            "methodParameters": { "calldata": "0x1234" }
        });
        let err =
            parse_liberty_quote_json(&json_no_src, req_amount, target, recipient).unwrap_err();
        assert!(err.to_string().contains("srcToken.address"));

        // 2. Missing destToken
        let json_no_dest = serde_json::json!({
            "to": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "destAmount": "5000",
            "srcToken": { "address": ARBITRUM_USDC_ADDR },
            "srcAmount": "1000",
            "approval": {
                "spender": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
                "calldata": "0x095ea7b3"
            },
            "methodParameters": { "calldata": "0x1234" }
        });
        let err =
            parse_liberty_quote_json(&json_no_dest, req_amount, target, recipient).unwrap_err();
        assert!(err.to_string().contains("destToken.address"));

        // 3. Missing srcAmount
        let json_no_amount = serde_json::json!({
            "to": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "destAmount": "5000",
            "srcToken": { "address": ARBITRUM_USDC_ADDR },
            "destToken": { "address": DEFAULT_BTC_MEME_ADDR },
            "approval": {
                "spender": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
                "calldata": "0x095ea7b3"
            },
            "methodParameters": { "calldata": "0x1234" }
        });
        let err =
            parse_liberty_quote_json(&json_no_amount, req_amount, target, recipient).unwrap_err();
        assert!(err.to_string().contains("srcAmount"));

        // 4. Missing approval spender
        let json_no_spender = serde_json::json!({
            "to": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "destAmount": "5000",
            "srcToken": { "address": ARBITRUM_USDC_ADDR },
            "destToken": { "address": DEFAULT_BTC_MEME_ADDR },
            "srcAmount": "1000",
            "approval": { "calldata": "0x095ea7b3" },
            "methodParameters": { "calldata": "0x1234" }
        });
        let err =
            parse_liberty_quote_json(&json_no_spender, req_amount, target, recipient).unwrap_err();
        assert!(err.to_string().contains("approval.spender"));

        // 5. Missing approval.calldata (reject fabricated approval)
        let json_no_calldata = serde_json::json!({
            "to": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "destAmount": "5000",
            "srcToken": { "address": ARBITRUM_USDC_ADDR },
            "destToken": { "address": DEFAULT_BTC_MEME_ADDR },
            "srcAmount": "1000",
            "approval": { "spender": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d" },
            "methodParameters": { "calldata": "0x1234" }
        });
        let err =
            parse_liberty_quote_json(&json_no_calldata, req_amount, target, recipient).unwrap_err();
        assert!(err.to_string().contains("approval.calldata"));

        // 6. Invalid approval.calldata hex
        let json_bad_calldata = serde_json::json!({
            "to": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "destAmount": "5000",
            "srcToken": { "address": ARBITRUM_USDC_ADDR },
            "destToken": { "address": DEFAULT_BTC_MEME_ADDR },
            "srcAmount": "1000",
            "approval": {
                "spender": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
                "calldata": "not_hex"
            },
            "methodParameters": { "calldata": "0x1234" }
        });
        let err = parse_liberty_quote_json(&json_bad_calldata, req_amount, target, recipient)
            .unwrap_err();
        assert!(err.to_string().contains("approval calldata hex"));
    }

    #[test]
    fn test_parse_liberty_quote_recipient_binding() {
        let req_amount = U256::from(1000);
        let target = Address::from_str(DEFAULT_BTC_MEME_ADDR).unwrap();
        let recipient = Address::from_str("0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63").unwrap();

        // ABI-encoded 32-byte word with 12 zero bytes prefix + 20 address bytes
        let mut recipient_word = [0u8; 32];
        recipient_word[12..32].copy_from_slice(recipient.as_slice());
        let valid_calldata = format!("0x12345678{}", hex::encode(recipient_word));

        let json_with_recipient = serde_json::json!({
            "to": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "destAmount": "5000",
            "srcToken": { "address": ARBITRUM_USDC_ADDR },
            "destToken": { "address": DEFAULT_BTC_MEME_ADDR },
            "srcAmount": "1000",
            "approval": {
                "spender": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
                "calldata": "0x095ea7b3"
            },
            "methodParameters": { "calldata": valid_calldata }
        });

        let quote = parse_liberty_quote_json(&json_with_recipient, req_amount, target, recipient)
            .expect("calldata containing recipient must be accepted");
        assert_eq!(quote.dest_amount, U256::from(5000));

        // Missing recipient in calldata
        let json_lacking_recipient = serde_json::json!({
            "to": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "destAmount": "5000",
            "srcToken": { "address": ARBITRUM_USDC_ADDR },
            "destToken": { "address": DEFAULT_BTC_MEME_ADDR },
            "srcAmount": "1000",
            "approval": {
                "spender": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
                "calldata": "0x095ea7b3"
            },
            "methodParameters": { "calldata": "0x123456780000000000000000000000000000000000000000000000000000000000000001" }
        });

        let err = parse_liberty_quote_json(&json_lacking_recipient, req_amount, target, recipient)
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("does not contain expected recipient"));
    }

    #[tokio::test]
    async fn test_zero_loss_gate_loss_then_profit_in_window_does_not_bank() {
        let config = AutoReinvestConfig {
            enabled: true,
            min_profit_threshold_usd: 25.0,
            ..Default::default()
        };
        // Manager initialized with epoch_start = 0 and recipient = ZERO (simulating unquoted evaluation)
        let mut mgr = SiphonManager::new(config, 0);

        // Trade 1 at day 1: loss of $100
        let fills_loss = vec![LiveTradeFill {
            oid: 1,
            coin: "BTC".into(),
            dir: "Close Long".into(),
            px: 70000.0,
            sz: 0.1,
            closed_pnl: -100.0,
            fee: 1.0,
            time_ms: 86_400_000, // day 1
        }];

        // Eval 1 after week 1: drawdown of -$101 -> Skipped
        let res1 = mgr
            .evaluate_and_siphon(&fills_loss, 8 * 86_400_000)
            .await
            .unwrap();
        assert!(res1.is_some());
        let r1 = res1.unwrap();
        assert!(matches!(r1.status, SiphonStatus::Skipped { .. }));
        // CRITICAL INVARIANT: epoch_start_ms MUST NOT have been reset to 8 * 86_400_000
        assert_eq!(
            mgr.epoch_start_ms.load(Ordering::Relaxed),
            0,
            "drawdown must not advance epoch_start_ms"
        );

        // Trade 2 at day 9: profit of $50
        let mut fills_both = fills_loss.clone();
        fills_both.push(LiveTradeFill {
            oid: 2,
            coin: "BTC".into(),
            dir: "Close Long".into(),
            px: 72000.0,
            sz: 0.1,
            closed_pnl: 50.0,
            fee: 1.0,
            time_ms: 9 * 86_400_000, // day 9
        });

        // Eval 2 after week 2: net PnL is -$101 + $49 = -$52 <= 0.
        // It must NOT bank the $50 profit because the prior loss in the window is retained!
        let res2 = mgr
            .evaluate_and_siphon(&fills_both, 15 * 86_400_000)
            .await
            .unwrap();
        assert!(res2.is_some());
        let r2 = res2.unwrap();
        match r2.status {
            SiphonStatus::Skipped { reason } => {
                assert!(
                    reason.contains("drawdown"),
                    "must skip due to retained drawdown, got: {reason}"
                );
            }
            other => panic!("expected Skipped drawdown status, got {other:?}"),
        }
        assert_eq!(
            mgr.epoch_start_ms.load(Ordering::Relaxed),
            0,
            "epoch_start_ms must still be 0 after second skip"
        );
    }

    #[test]
    fn test_allowlist_rejection_of_unknown_router() {
        let unknown_router =
            Address::from_str("0x000000000000000000000000000000000000dead").unwrap();
        assert!(!is_whitelisted_router(unknown_router));

        let official_router =
            Address::from_str("0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d").unwrap();
        assert!(is_whitelisted_router(official_router));

        let recipient = Address::from_str("0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63").unwrap();
        let target = Address::from_str(DEFAULT_BTC_MEME_ADDR).unwrap();

        // Rejection in parse_liberty_quote_json
        let bad_router_json = serde_json::json!({
            "to": "0x000000000000000000000000000000000000dead",
            "destAmount": "1000000000",
            "methodParameters": { "calldata": "0x1234" }
        });
        let err = parse_liberty_quote_json(&bad_router_json, U256::from(1000), target, recipient)
            .unwrap_err();
        assert!(err.to_string().contains("not on allowlist"));

        // Spender rejection
        let bad_spender_json = serde_json::json!({
            "to": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "destAmount": "1000000000",
            "srcToken": { "address": ARBITRUM_USDC_ADDR },
            "destToken": { "address": DEFAULT_BTC_MEME_ADDR },
            "srcAmount": "1000",
            "approval": {
                "spender": "0x000000000000000000000000000000000000dead",
                "calldata": "0x095ea7b3"
            },
            "methodParameters": { "calldata": "0x1234" }
        });
        let err = parse_liberty_quote_json(&bad_spender_json, U256::from(1000), target, recipient)
            .unwrap_err();
        assert!(err.to_string().contains("not on allowlist"));
    }

    #[test]
    fn test_pending_exec_path_and_state_live_under_wallet_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        let old_xdg = std::env::var("XDG_DATA_HOME").ok();

        std::env::set_var("HOME", dir.path());
        std::env::remove_var("XDG_DATA_HOME");

        let pending_path = pending_exec_path();
        let state_path = siphon_state_path();

        assert!(pending_path.is_absolute());
        assert!(state_path.is_absolute());
        assert_eq!(
            pending_path,
            dir.path()
                .join(".local/share/vaughan-cli/siphon_pending.json")
        );
        assert_eq!(
            state_path,
            dir.path()
                .join(".local/share/vaughan-cli/siphon_state.json")
        );

        // Restore environment
        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        }
        if let Some(x) = old_xdg {
            std::env::set_var("XDG_DATA_HOME", x);
        }
    }

    #[test]
    fn test_siphon_window_state_persistence_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("siphon_state.json");

        let state = SiphonWindowState {
            epoch_start_ms: 1730000000000,
            last_siphon_ms: 1730500000000,
        };

        save_siphon_state_to_path(&state_file, &state);
        let loaded = load_siphon_state_from_path(&state_file).expect("load state");
        assert_eq!(loaded, state);

        // Verify SiphonManager::new_with_state loads persisted window
        let config = AutoReinvestConfig::default();
        let mgr = SiphonManager::new_with_state(
            config,
            loaded.epoch_start_ms,
            loaded.last_siphon_ms,
            Address::ZERO,
        );
        assert_eq!(mgr.epoch_start_ms.load(Ordering::Relaxed), 1730000000000);
        assert_eq!(mgr.last_siphon_ms.load(Ordering::Relaxed), 1730500000000);
    }

    #[test]
    fn test_pending_execution_json_shape_round_trip() {
        let pending = SiphonPending {
            created_ms: 1730000000000,
            recipient: Address::from_str("0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63").unwrap(),
            chain_id: 42161,
            amount_usdc: U256::from(10_000_000u64),
            target_symbol: "BTC (meme)".to_string(),
            target_token: DEFAULT_BTC_MEME_ADDR.to_string(),
            router: Address::from_str("0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d").unwrap(),
            approval_spender: Address::from_str("0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d")
                .unwrap(),
            approval_calldata: "0x095ea7b3".to_string(),
            bridge_calldata: "0x779b432d".to_string(),
            bridge_value: U256::ZERO,
            dest_amount: U256::from(358806120622451933052928u128),
            fee_percent: 0.30,
            net_profit_usd: 50.0,
            profit_pct: 20.0,
        };

        let serialized = serde_json::to_string(&pending).expect("serialize pending");
        let deserialized: SiphonPending =
            serde_json::from_str(&serialized).expect("deserialize pending");
        assert_eq!(deserialized, pending);
    }

    #[test]
    fn test_non_dry_run_receipt_never_contains_placeholder_hash() {
        let receipt = SiphonReceipt {
            timestamp_ms: 1000,
            net_profit_usd: 100.0,
            profit_pct: 20.0,
            siphon_usd: 20.0,
            target_token: DEFAULT_BTC_MEME_ADDR.to_string(),
            target_symbol: "BTC (meme)".to_string(),
            winning_aggregator: Some("SquirrelSwap".to_string()),
            route_description: Some("9mm V2".to_string()),
            amount_out_tokens: Some("200".to_string()),
            status: SiphonStatus::Quoted,
            intent: None,
            dry_run: false,
        };

        // Confirm SiphonStatus has no synthetic hash
        assert_eq!(receipt.status, SiphonStatus::Quoted);
        let serialized = serde_json::to_string(&receipt).unwrap();
        assert!(!serialized.contains("0xSIM_"));
        assert!(!serialized.contains("0xDRY_RUN"));
    }

    #[test]
    fn test_siphon_receipt_serde_round_trip() {
        let quoted = SiphonReceipt {
            timestamp_ms: 1730000000000,
            net_profit_usd: 120.0,
            profit_pct: 20.0,
            siphon_usd: 24.0,
            target_token: DEFAULT_BTC_MEME_ADDR.to_string(),
            target_symbol: "BTC (meme)".to_string(),
            winning_aggregator: Some("SquirrelSwap".to_string()),
            route_description: Some("9mm V2".to_string()),
            amount_out_tokens: Some("1500000".to_string()),
            status: SiphonStatus::Quoted,
            intent: Some(SiphonIntent {
                token_in: Address::ZERO,
                token_out: Address::ZERO,
                amount_in: U256::from(100),
                slippage_bps: 100,
                chain_id: 369,
                winning_venue: "SquirrelSwap".to_string(),
                route_description: "9mm V2".to_string(),
                amount_out_tokens: "1500000".to_string(),
            }),
            dry_run: true,
        };
        let json_quoted = serde_json::to_string(&quoted).expect("serialize quoted");
        let de_quoted: SiphonReceipt =
            serde_json::from_str(&json_quoted).expect("deserialize quoted");
        assert_eq!(de_quoted, quoted);

        let skipped = SiphonReceipt {
            timestamp_ms: 1730000000000,
            net_profit_usd: 2.0,
            profit_pct: 20.0,
            siphon_usd: 0.0,
            target_token: DEFAULT_BTC_MEME_ADDR.to_string(),
            target_symbol: "BTC (meme)".to_string(),
            winning_aggregator: None,
            route_description: None,
            amount_out_tokens: None,
            status: SiphonStatus::Skipped {
                reason: "drawdown: net PnL <= 0".to_string(),
            },
            intent: None,
            dry_run: true,
        };
        let json_skipped = serde_json::to_string(&skipped).expect("serialize skipped");
        let de_skipped: SiphonReceipt =
            serde_json::from_str(&json_skipped).expect("deserialize skipped");
        assert_eq!(de_skipped, skipped);
    }

    #[test]
    fn test_persisted_jsonl_quoted_receipt_contains_no_tx_hash_or_placeholders() {
        let quoted = SiphonReceipt {
            timestamp_ms: 1730000000000,
            net_profit_usd: 50.0,
            profit_pct: 20.0,
            siphon_usd: 10.0,
            target_token: DEFAULT_BTC_MEME_ADDR.to_string(),
            target_symbol: "BTC (meme)".to_string(),
            winning_aggregator: Some("SquirrelSwap".to_string()),
            route_description: Some("direct route".to_string()),
            amount_out_tokens: Some("420000".to_string()),
            status: SiphonStatus::Quoted,
            intent: None,
            dry_run: false,
        };
        let line = serde_json::to_string(&quoted).expect("serialize");
        assert!(
            !line.contains("tx_hash"),
            "JSON must not contain tx_hash field: {line}"
        );
        assert!(
            !line.contains("0xSIM_"),
            "JSON must not contain 0xSIM_ placeholder: {line}"
        );
        assert!(
            !line.contains("0xDRY_RUN"),
            "JSON must not contain 0xDRY_RUN placeholder: {line}"
        );
        assert!(
            !line.contains("placeholder"),
            "JSON must not contain placeholder string: {line}"
        );
    }
}
