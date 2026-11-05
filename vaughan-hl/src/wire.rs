//! Hyperliquid L1 wire formatting, minimal MsgPack encoder, monotonic nonce
//! allocator, and EIP-712 Phantom-Agent & `ApproveAgent` signing (`HL-1`).
//!
//! Built exclusively on `Vaughan-CLI` allowlisted crates (`alloy`, `alloy-dyn-abi`,
//! `hex`) with a zero-dependency deterministic MsgPack encoder for the fixed
//! Hyperliquid L1 action schemas (`order`, `cancel`, `updateLeverage`).

use std::sync::Mutex;

use alloy::primitives::{keccak256, Address, B256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use alloy_dyn_abi::TypedData;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::HlError;

/// Maximum builder fee in tenths of a basis point (`100 = 10 bps = 0.10%` perps cap).
pub const MAX_PERPS_BUILDER_FEE_TENTHS_BP: u32 = 100;

/// Resolve the numeric Hyperliquid asset ID (`a` field on the wire).
///
/// - Standard native crypto perps (`dex_index == 0`): `index_in_meta`
/// - HIP-3 builder-deployed perps (`dex_index >= 1`, e.g. `xyz:CL`, `xyz:GOLD`, `xyz:NVDA`):
///   `100_000 + dex_index * 10_000 + index_in_meta`
pub const fn resolve_asset_id(dex_index: u32, index_in_meta: u32) -> u32 {
    if dex_index == 0 {
        index_in_meta
    } else {
        100_000 + dex_index * 10_000 + index_in_meta
    }
}

/// Format an order size (`sz`) according to Hyperliquid lot-size rules:
/// floors to `sz_decimals` decimal places (never rounds up past available margin)
/// and strips trailing zeros.
pub fn format_sz(sz: f64, sz_decimals: u32) -> Result<String, HlError> {
    if !sz.is_finite() || sz <= 0.0 {
        return Err(HlError::Wire(format!(
            "order size must be finite and > 0 (got {sz})"
        )));
    }
    if sz_decimals > 8 {
        return Err(HlError::Wire(format!(
            "sz_decimals {sz_decimals} exceeds maximum supported precision (8)"
        )));
    }
    let factor = 10_f64.powi(sz_decimals as i32);
    // Small epsilon guards against binary float representation like `0.29999999999999999`.
    let floored = ((sz * factor) + 1e-9).floor() / factor;
    if floored <= 0.0 {
        return Err(HlError::Wire(format!(
            "order size {sz} rounds down to 0 at sz_decimals={sz_decimals}"
        )));
    }
    let formatted = format!("{floored:.prec$}", prec = sz_decimals as usize);
    Ok(strip_trailing_zeros(&formatted))
}

/// Format a limit/trigger price (`px`) according to Hyperliquid Perps tick rules:
/// - Integer prices are always allowed regardless of significant figures.
/// - Non-integer prices are rounded to at most `5` significant figures AND at most
///   `6 - sz_decimals` decimal places, with trailing zeros stripped.
pub fn format_px(px: f64, sz_decimals: u32) -> Result<String, HlError> {
    if !px.is_finite() || px <= 0.0 {
        return Err(HlError::Wire(format!(
            "price must be finite and > 0 (got {px})"
        )));
    }
    let max_decimals = 6_u32.saturating_sub(sz_decimals);

    // Integer prices >= 100_000 are represented as exact integers on Hyperliquid.
    if px >= 100_000.0 {
        return Ok(format!("{:.0}", px.round()));
    }

    // Round to 5 significant figures first.
    let mag = px.log10().floor() as i32;
    let sig_decimals = (4 - mag).max(0) as u32;
    let decimals = sig_decimals.min(max_decimals);
    let factor = 10_f64.powi(decimals as i32);
    let rounded = (px * factor).round() / factor;
    if rounded <= 0.0 {
        return Err(HlError::Wire(format!(
            "price {px} rounded to 0 at sz_decimals={sz_decimals}"
        )));
    }
    let formatted = format!("{rounded:.prec$}", prec = decimals as usize);
    Ok(strip_trailing_zeros(&formatted))
}

fn strip_trailing_zeros(s: &str) -> String {
    if let Some((int_part, frac_part)) = s.split_once('.') {
        let trimmed = frac_part.trim_end_matches('0');
        if trimmed.is_empty() {
            int_part.to_string()
        } else {
            format!("{int_part}.{trimmed}")
        }
    } else {
        s.to_string()
    }
}

/// Thread-safe monotonic nonce allocator for Hyperliquid L1 actions.
///
/// Guarantees strictly increasing nonces (`next = (last + 1).max(now_ms)`) so
/// entry + SL + T1 + TP orders emitted in the same millisecond tick never collide.
#[derive(Debug)]
pub struct NonceAllocator {
    last: Mutex<u64>,
}

impl NonceAllocator {
    pub const fn new(initial: u64) -> Self {
        Self {
            last: Mutex::new(initial),
        }
    }

    /// Allocate the next strictly increasing nonce for timestamp `now_ms`.
    pub fn next_nonce(&self, now_ms: u64) -> u64 {
        let mut guard = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let next = guard.saturating_add(1).max(now_ms);
        *guard = next;
        next
    }
}

/// Time-in-force for limit orders on the Hyperliquid wire (`t.limit.tif`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireTif {
    /// Add Liquidity Only (post-only).
    Alo,
    /// Immediate-Or-Cancel (marketable limit).
    Ioc,
    /// Good-Til-Cancelled.
    Gtc,
}

impl WireTif {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alo => "Alo",
            Self::Ioc => "Ioc",
            Self::Gtc => "Gtc",
        }
    }
}

/// Take-Profit or Stop-Loss trigger tag (`t.trigger.tpsl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireTpSl {
    Tp,
    Sl,
}

impl WireTpSl {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tp => "tp",
            Self::Sl => "sl",
        }
    }
}

/// Order type field (`t`) on the Hyperliquid L1 wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderTypeWire {
    Limit {
        tif: WireTif,
    },
    Trigger {
        is_market: bool,
        trigger_px: String,
        tpsl: WireTpSl,
    },
}

/// Single order wire payload (`a, b, p, s, r, t, c`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderWire {
    /// Asset ID (`a`).
    pub a: u32,
    /// `true` for Buy/Long, `false` for Sell/Short (`b`).
    pub b: bool,
    /// Formatted limit price (`p`).
    pub p: String,
    /// Formatted order size (`s`).
    pub s: String,
    /// Reduce-only flag (`r`).
    pub r: bool,
    /// Order type (`t`: limit or trigger).
    pub t: OrderTypeWire,
    /// Optional 128-bit hex client order ID (`c`: `"0x"` + 32 lowercase hex chars).
    pub c: Option<String>,
}

/// Optional Hyperliquid builder code fee attachment (`builder: { b, f }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderFeeWire {
    /// Lowercase `0x`-prefixed builder address (`b`).
    pub b: String,
    /// Builder fee in tenths of a basis point (`f`, e.g. `10` = 1 bp).
    pub f: u32,
}

/// Grouping mode for an order batch (`na`, `normalTpsl`, `positionTpsl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderGrouping {
    Na,
    NormalTpsl,
    PositionTpsl,
}

impl OrderGrouping {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Na => "na",
            Self::NormalTpsl => "normalTpsl",
            Self::PositionTpsl => "positionTpsl",
        }
    }
}

/// Supported Hyperliquid L1 actions hashed into `connectionId` via MsgPack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum L1ActionWire {
    Order {
        orders: Vec<OrderWire>,
        grouping: OrderGrouping,
        builder: Option<BuilderFeeWire>,
    },
    Cancel {
        /// `(asset_id, oid)` pairs.
        cancels: Vec<(u32, u64)>,
    },
    UpdateLeverage {
        asset: u32,
        is_cross: bool,
        leverage: u32,
    },
}

// ============================================================================
// Zero-dependency deterministic MsgPack encoder for Hyperliquid L1 actions
// ============================================================================

fn mp_write_map_len(buf: &mut Vec<u8>, len: usize) {
    if len <= 15 {
        buf.push(0x80 | (len as u8));
    } else {
        buf.push(0xde);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    }
}

fn mp_write_array_len(buf: &mut Vec<u8>, len: usize) {
    if len <= 15 {
        buf.push(0x90 | (len as u8));
    } else {
        buf.push(0xdc);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    }
}

fn mp_write_str(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len <= 31 {
        buf.push(0xa0 | (len as u8));
    } else if len <= 255 {
        buf.push(0xd9);
        buf.push(len as u8);
    } else {
        buf.push(0xda);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    }
    buf.extend_from_slice(bytes);
}

fn mp_write_bool(buf: &mut Vec<u8>, v: bool) {
    buf.push(if v { 0xc3 } else { 0xc2 });
}

fn mp_write_uint(buf: &mut Vec<u8>, v: u64) {
    if v <= 0x7f {
        buf.push(v as u8);
    } else if v <= 0xff {
        buf.push(0xcc);
        buf.push(v as u8);
    } else if v <= 0xffff {
        buf.push(0xcd);
        buf.extend_from_slice(&(v as u16).to_be_bytes());
    } else if v <= 0xffff_ffff {
        buf.push(0xce);
        buf.extend_from_slice(&(v as u32).to_be_bytes());
    } else {
        buf.push(0xcf);
        buf.extend_from_slice(&v.to_be_bytes());
    }
}

fn mp_write_order(buf: &mut Vec<u8>, o: &OrderWire) {
    let field_count = if o.c.is_some() { 7 } else { 6 };
    mp_write_map_len(buf, field_count);

    mp_write_str(buf, "a");
    mp_write_uint(buf, u64::from(o.a));

    mp_write_str(buf, "b");
    mp_write_bool(buf, o.b);

    mp_write_str(buf, "p");
    mp_write_str(buf, &o.p);

    mp_write_str(buf, "s");
    mp_write_str(buf, &o.s);

    mp_write_str(buf, "r");
    mp_write_bool(buf, o.r);

    mp_write_str(buf, "t");
    match &o.t {
        OrderTypeWire::Limit { tif } => {
            mp_write_map_len(buf, 1);
            mp_write_str(buf, "limit");
            mp_write_map_len(buf, 1);
            mp_write_str(buf, "tif");
            mp_write_str(buf, tif.as_str());
        }
        OrderTypeWire::Trigger {
            is_market,
            trigger_px,
            tpsl,
        } => {
            mp_write_map_len(buf, 1);
            mp_write_str(buf, "trigger");
            mp_write_map_len(buf, 3);
            mp_write_str(buf, "isMarket");
            mp_write_bool(buf, *is_market);
            mp_write_str(buf, "triggerPx");
            mp_write_str(buf, trigger_px);
            mp_write_str(buf, "tpsl");
            mp_write_str(buf, tpsl.as_str());
        }
    }

    if let Some(cloid) = &o.c {
        mp_write_str(buf, "c");
        mp_write_str(buf, cloid);
    }
}

impl L1ActionWire {
    /// Validate all fields of this L1 action before encoding or signing
    /// (`Vaughan-CLI` Security Guardrail #5: validate before you sign or broadcast).
    pub fn validate(&self) -> Result<(), HlError> {
        let valid_pos_dec = |s: &str| -> bool {
            !s.is_empty()
                && s.len() <= 32
                && s.parse::<f64>().is_ok_and(|v| v.is_finite() && v > 0.0)
        };
        match self {
            Self::Order {
                orders, builder, ..
            } => {
                if orders.is_empty() || orders.len() > 64 {
                    return Err(HlError::Wire(format!(
                        "orders batch size must be in 1..=64 (got {})",
                        orders.len()
                    )));
                }
                for o in orders {
                    if !valid_pos_dec(&o.p) {
                        return Err(HlError::Wire(format!("invalid order price '{}'", o.p)));
                    }
                    if !valid_pos_dec(&o.s) {
                        return Err(HlError::Wire(format!("invalid order size '{}'", o.s)));
                    }
                    if let OrderTypeWire::Trigger { trigger_px, .. } = &o.t {
                        if !valid_pos_dec(trigger_px) {
                            return Err(HlError::Wire(format!("invalid triggerPx '{trigger_px}'")));
                        }
                    }
                    if let Some(cloid) = &o.c {
                        let ok = cloid.len() == 34
                            && cloid.starts_with("0x")
                            && cloid[2..]
                                .chars()
                                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
                        if !ok {
                            return Err(HlError::Wire(format!(
                                "cloid '{cloid}' must be 0x followed by 32 lowercase hex chars"
                            )));
                        }
                    }
                }
                if let Some(b) = builder {
                    if b.f > MAX_PERPS_BUILDER_FEE_TENTHS_BP {
                        return Err(HlError::Wire(format!(
                            "builder fee {} exceeds max perps cap ({})",
                            b.f, MAX_PERPS_BUILDER_FEE_TENTHS_BP
                        )));
                    }
                    let addr = b
                        .b
                        .parse::<Address>()
                        .map_err(|_| HlError::Wire(format!("invalid builder address '{}'", b.b)))?;
                    if addr == Address::ZERO {
                        return Err(HlError::Wire(
                            "builder address cannot be the zero address".to_string(),
                        ));
                    }
                }
            }
            Self::Cancel { cancels } => {
                if cancels.is_empty() || cancels.len() > 64 {
                    return Err(HlError::Wire(format!(
                        "cancels batch size must be in 1..=64 (got {})",
                        cancels.len()
                    )));
                }
            }
            Self::UpdateLeverage { leverage, .. } => {
                if *leverage == 0 || *leverage > 50 {
                    return Err(HlError::Wire(format!(
                        "leverage must be between 1 and 50 (got {leverage})"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Encode this L1 action into exact Hyperliquid canonical MsgPack bytes
    /// (preserving exact field insertion order expected by the L1 verifier).
    pub fn to_msgpack_bytes(&self) -> Result<Vec<u8>, HlError> {
        self.validate()?;
        let mut buf = Vec::with_capacity(128);
        match self {
            Self::Order {
                orders,
                grouping,
                builder,
            } => {
                let map_len = if builder.is_some() { 4 } else { 3 };
                mp_write_map_len(&mut buf, map_len);

                mp_write_str(&mut buf, "type");
                mp_write_str(&mut buf, "order");

                mp_write_str(&mut buf, "orders");
                mp_write_array_len(&mut buf, orders.len());
                for o in orders {
                    mp_write_order(&mut buf, o);
                }

                mp_write_str(&mut buf, "grouping");
                mp_write_str(&mut buf, grouping.as_str());

                if let Some(b) = builder {
                    mp_write_str(&mut buf, "builder");
                    mp_write_map_len(&mut buf, 2);
                    mp_write_str(&mut buf, "b");
                    mp_write_str(&mut buf, &b.b.to_ascii_lowercase());
                    mp_write_str(&mut buf, "f");
                    mp_write_uint(&mut buf, u64::from(b.f));
                }
            }
            Self::Cancel { cancels } => {
                mp_write_map_len(&mut buf, 2);
                mp_write_str(&mut buf, "type");
                mp_write_str(&mut buf, "cancel");
                mp_write_str(&mut buf, "cancels");
                mp_write_array_len(&mut buf, cancels.len());
                for &(asset, oid) in cancels {
                    mp_write_map_len(&mut buf, 2);
                    mp_write_str(&mut buf, "a");
                    mp_write_uint(&mut buf, u64::from(asset));
                    mp_write_str(&mut buf, "o");
                    mp_write_uint(&mut buf, oid);
                }
            }
            Self::UpdateLeverage {
                asset,
                is_cross,
                leverage,
            } => {
                mp_write_map_len(&mut buf, 4);
                mp_write_str(&mut buf, "type");
                mp_write_str(&mut buf, "updateLeverage");
                mp_write_str(&mut buf, "asset");
                mp_write_uint(&mut buf, u64::from(*asset));
                mp_write_str(&mut buf, "isCross");
                mp_write_bool(&mut buf, *is_cross);
                mp_write_str(&mut buf, "leverage");
                mp_write_uint(&mut buf, u64::from(*leverage));
            }
        }
        Ok(buf)
    }

    /// Compute the Hyperliquid L1 `connectionId` digest:
    /// `keccak256(msgpack(action) || nonce_be8 || (0x00 if vault.is_none() else 0x01 || vault_addr20))`.
    pub fn connection_id(
        &self,
        nonce: u64,
        vault_address: Option<Address>,
    ) -> Result<B256, HlError> {
        let mut payload = self.to_msgpack_bytes()?;
        payload.extend_from_slice(&nonce.to_be_bytes());
        match vault_address {
            None => payload.push(0x00),
            Some(addr) => {
                payload.push(0x01);
                payload.extend_from_slice(addr.as_slice());
            }
        }
        Ok(keccak256(&payload))
    }
}

impl L1ActionWire {
    /// JSON `action` object for `POST /exchange` (key order is irrelevant to the
    /// server; the signed MsgPack form is produced by [`Self::to_msgpack_bytes`]).
    pub fn to_json_action(&self) -> serde_json::Value {
        match self {
            Self::Order {
                orders,
                grouping,
                builder,
            } => {
                let ords: Vec<serde_json::Value> = orders
                    .iter()
                    .map(|o| {
                        let t = match &o.t {
                            OrderTypeWire::Limit { tif } => {
                                json!({ "limit": { "tif": tif.as_str() } })
                            }
                            OrderTypeWire::Trigger {
                                is_market,
                                trigger_px,
                                tpsl,
                            } => json!({ "trigger": {
                                "isMarket": is_market,
                                "triggerPx": trigger_px,
                                "tpsl": tpsl.as_str()
                            } }),
                        };
                        let mut v = json!({
                            "a": o.a, "b": o.b, "p": o.p, "s": o.s, "r": o.r, "t": t
                        });
                        if let Some(c) = &o.c {
                            v["c"] = json!(c);
                        }
                        v
                    })
                    .collect();
                let mut act = json!({
                    "type": "order",
                    "orders": ords,
                    "grouping": grouping.as_str()
                });
                if let Some(b) = builder {
                    act["builder"] = json!({ "b": b.b.to_ascii_lowercase(), "f": b.f });
                }
                act
            }
            Self::Cancel { cancels } => json!({
                "type": "cancel",
                "cancels": cancels
                    .iter()
                    .map(|(a, o)| json!({ "a": a, "o": o }))
                    .collect::<Vec<_>>()
            }),
            Self::UpdateLeverage {
                asset,
                is_cross,
                leverage,
            } => json!({
                "type": "updateLeverage",
                "asset": asset,
                "isCross": is_cross,
                "leverage": leverage
            }),
        }
    }
}

/// Split `r, s, v` signature expected by Hyperliquid's `POST /exchange` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HlSignature {
    pub r: String,
    pub s: String,
    pub v: u8,
}

impl HlSignature {
    /// Parse an Ethereum 65-byte hex signature (0x-prefixed, 130 or 132 chars) into `HlSignature`.
    pub fn from_hex(sig_str: &str) -> Result<Self, HlError> {
        let trimmed = sig_str.trim();
        let stripped = trimmed.strip_prefix("0x").unwrap_or(trimmed);
        if stripped.len() != 130 {
            return Err(HlError::Signing(format!(
                "invalid signature length: expected 130 hex characters (65 bytes), got {}",
                stripped.len()
            )));
        }
        let r = format!("0x{}", &stripped[0..64]);
        let s = format!("0x{}", &stripped[64..128]);
        let mut v = u8::from_str_radix(&stripped[128..130], 16)
            .map_err(|e| HlError::Signing(format!("invalid signature v byte: {e}")))?;
        if v < 27 {
            v += 27;
        }
        Ok(Self { r, s, v })
    }
}

/// Sign an L1 action (`order`, `cancel`, `updateLeverage`) using the Phantom-Agent
/// EIP-712 schema (`Exchange`, `version = "1"`, `chainId = 1337`, `source = "a"` mainnet / `"b"` testnet).
pub fn sign_l1_action(
    agent_signer: &PrivateKeySigner,
    action: &L1ActionWire,
    nonce: u64,
    vault_address: Option<Address>,
    is_mainnet: bool,
) -> Result<HlSignature, HlError> {
    if nonce < 1_600_000_000_000 {
        return Err(HlError::Signing(format!(
            "nonce {nonce} is invalid (must be a unix millisecond timestamp)"
        )));
    }
    if vault_address == Some(Address::ZERO) {
        return Err(HlError::Signing(
            "vault_address cannot be Some(0x0000...0000); pass None for EOA/sub-account signing"
                .to_string(),
        ));
    }
    let conn_id = action.connection_id(nonce, vault_address)?;
    let source = if is_mainnet { "a" } else { "b" };
    let typed_json = json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "version", "type": "string" },
                { "name": "chainId", "type": "uint256" },
                { "name": "verifyingContract", "type": "address" }
            ],
            "Agent": [
                { "name": "source", "type": "string" },
                { "name": "connectionId", "type": "bytes32" }
            ]
        },
        "primaryType": "Agent",
        "domain": {
            "name": "Exchange",
            "version": "1",
            "chainId": 1337,
            "verifyingContract": "0x0000000000000000000000000000000000000000"
        },
        "message": {
            "source": source,
            "connectionId": format!("0x{}", hex::encode(conn_id.as_slice()))
        }
    });

    let typed_data: TypedData = serde_json::from_value(typed_json)
        .map_err(|e| HlError::Signing(format!("invalid Agent EIP-712 schema: {e}")))?;
    let digest = typed_data
        .eip712_signing_hash()
        .map_err(|e| HlError::Signing(format!("EIP-712 hash failed: {e}")))?;
    let sig = agent_signer
        .sign_hash_sync(&digest)
        .map_err(|e| HlError::Signing(e.to_string()))?;
    let raw = sig.as_bytes();
    let v = if raw[64] < 27 { raw[64] + 27 } else { raw[64] };
    Ok(HlSignature {
        r: format!("0x{}", hex::encode(&raw[0..32])),
        s: format!("0x{}", hex::encode(&raw[32..64])),
        v,
    })
}

/// Build the `eth_signTypedData_v4` JSON payload for `HyperliquidSignTransaction:ApproveAgent`.
///
/// The master / dedicated sub-account wallet signs this payload **once** via:
/// `vaughan sign-typed-data --file approve_agent.json`
/// so the master private key never enters the auto-trader process.
/// Pass `Address::ZERO` as `agent_address` to generate an emergency **Revoke Agent** payload.
pub fn build_approve_agent_typed_data(
    agent_address: Address,
    agent_name: Option<&str>,
    nonce: u64,
    is_mainnet: bool,
) -> serde_json::Value {
    let (chain_tag, sig_chain_id_hex, domain_chain_id) = if is_mainnet {
        ("Mainnet", "0xa4b1", 42161u64)
    } else {
        ("Testnet", "0x66eee", 421614u64)
    };
    let name_str = agent_name.unwrap_or("");
    json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "version", "type": "string" },
                { "name": "chainId", "type": "uint256" },
                { "name": "verifyingContract", "type": "address" }
            ],
            "HyperliquidTransaction:ApproveAgent": [
                { "name": "hyperliquidChain", "type": "string" },
                { "name": "agentAddress", "type": "address" },
                { "name": "agentName", "type": "string" },
                { "name": "nonce", "type": "uint64" }
            ]
        },
        "primaryType": "HyperliquidTransaction:ApproveAgent",
        "domain": {
            "name": "HyperliquidSignTransaction",
            "version": "1",
            "chainId": domain_chain_id,
            "verifyingContract": "0x0000000000000000000000000000000000000000"
        },
        "message": {
            "hyperliquidChain": chain_tag,
            "signatureChainId": sig_chain_id_hex,
            "agentAddress": format!("{agent_address:#x}"),
            "agentName": name_str,
            "nonce": nonce
        }
    })
}

/// Build the `POST /exchange` body for an `approveAgent` action.
pub fn build_approve_agent_exchange_payload(
    agent_address: Address,
    agent_name: Option<&str>,
    nonce: u64,
    sig: &HlSignature,
    is_mainnet: bool,
) -> serde_json::Value {
    let (chain_tag, sig_chain_id_hex) = if is_mainnet {
        ("Mainnet", "0xa4b1")
    } else {
        ("Testnet", "0x66eee")
    };
    json!({
        "action": {
            "type": "approveAgent",
            "hyperliquidChain": chain_tag,
            "signatureChainId": sig_chain_id_hex,
            "agentAddress": format!("{agent_address:#x}"),
            "agentName": agent_name.unwrap_or(""),
            "nonce": nonce
        },
        "nonce": nonce,
        "signature": {
            "r": sig.r,
            "s": sig.s,
            "v": sig.v
        }
    })
}

/// Broadcast an `approveAgent` action to Hyperliquid's exchange endpoint.
pub async fn post_approve_agent_registration(
    exchange_url: &str,
    agent_address: Address,
    agent_name: Option<&str>,
    nonce: u64,
    sig: &HlSignature,
    is_mainnet: bool,
) -> Result<serde_json::Value, HlError> {
    if !exchange_url.starts_with("https://") {
        return Err(HlError::Api(
            "Hyperliquid exchange_url must use https://".to_string(),
        ));
    }
    let payload =
        build_approve_agent_exchange_payload(agent_address, agent_name, nonce, sig, is_mainnet);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| HlError::Api(e.to_string()))?;

    let resp = client
        .post(exchange_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| HlError::Api(format!("network error connecting to Hyperliquid: {e}")))?;

    let status = resp.status();
    let resp_text = resp
        .text()
        .await
        .unwrap_or_else(|_| "<unreadable response>".to_string());

    if !status.is_success() {
        return Err(HlError::Api(format!(
            "Hyperliquid /exchange returned HTTP {status}: {resp_text}"
        )));
    }

    let parsed: serde_json::Value = serde_json::from_str(&resp_text)
        .map_err(|e| HlError::Api(format!("malformed JSON response: {e} ({resp_text})")))?;

    if let Some(err_msg) = parsed.get("response").and_then(|r| r.as_str()) {
        if err_msg.contains("err") || err_msg.contains("Err") {
            return Err(HlError::Api(err_msg.to_string()));
        }
    }
    if let Some(status_str) = parsed.get("status").and_then(|s| s.as_str()) {
        if status_str.eq_ignore_ascii_case("err") {
            let msg = parsed
                .get("response")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown exchange error");
            return Err(HlError::Api(msg.to_string()));
        }
    }

    Ok(parsed)
}

/// Sign-agnostic broadcast of an already-signed L1 action (`order`, `cancel`,
/// `updateLeverage`) to Hyperliquid's `POST /exchange`. The agent key signs on
/// behalf of its approved backing account, so no `vaultAddress` is attached.
pub async fn post_l1_action(
    exchange_url: &str,
    action: &L1ActionWire,
    nonce: u64,
    sig: &HlSignature,
) -> Result<serde_json::Value, HlError> {
    if !exchange_url.starts_with("https://") {
        return Err(HlError::Api(
            "Hyperliquid exchange_url must use https://".to_string(),
        ));
    }
    action.validate()?;
    let payload = json!({
        "action": action.to_json_action(),
        "nonce": nonce,
        "signature": { "r": sig.r, "s": sig.s, "v": sig.v }
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| HlError::Api(e.to_string()))?;
    let resp = client
        .post(exchange_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| HlError::Api(format!("network error connecting to Hyperliquid: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .unwrap_or_else(|_| "<unreadable response>".to_string());
    if !status.is_success() {
        return Err(HlError::Api(format!(
            "Hyperliquid /exchange returned HTTP {status}: {text}"
        )));
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| HlError::Api(format!("malformed JSON response: {e} ({text})")))?;
    if parsed
        .get("status")
        .and_then(|s| s.as_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("err"))
    {
        let msg = parsed
            .get("response")
            .map(|r| r.to_string())
            .unwrap_or_else(|| "unknown exchange error".into());
        return Err(HlError::Api(msg));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hl_signature_from_hex_roundtrips_and_validates() {
        let sig_hex = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
                       bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\
                       1b";
        let parsed = HlSignature::from_hex(sig_hex).unwrap();
        assert_eq!(
            parsed.r,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            parsed.s,
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(parsed.v, 27);

        // Normalize 0 or 1 into 27 or 28
        let sig_v0 = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
                      bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\
                      00";
        assert_eq!(HlSignature::from_hex(sig_v0).unwrap().v, 27);

        // Reject invalid length
        assert!(HlSignature::from_hex("0xdeadbeef").is_err());
    }

    #[test]
    fn builds_approve_agent_exchange_payload_cleanly() {
        let sig = HlSignature {
            r: "0x1111111111111111111111111111111111111111111111111111111111111111".into(),
            s: "0x2222222222222222222222222222222222222222222222222222222222222222".into(),
            v: 28,
        };
        let payload = build_approve_agent_exchange_payload(
            Address::repeat_byte(0x42),
            Some("vaughan-agent"),
            1730000000000,
            &sig,
            false,
        );
        assert_eq!(payload["action"]["type"], "approveAgent");
        assert_eq!(payload["action"]["hyperliquidChain"], "Testnet");
        assert_eq!(payload["action"]["signatureChainId"], "0x66eee");
        assert_eq!(payload["action"]["agentName"], "vaughan-agent");
        assert_eq!(payload["signature"]["v"], 28);
    }

    #[test]
    fn format_sz_floors_and_strips_trailing_zeros() {
        // Floors rather than rounding up past available margin
        assert_eq!(format_sz(1.239, 2).unwrap(), "1.23");
        assert_eq!(format_sz(1.500, 3).unwrap(), "1.5");
        assert_eq!(format_sz(10.0, 4).unwrap(), "10");
        assert!(format_sz(0.0001, 2).is_err());
    }

    #[test]
    fn format_px_respects_5_sig_figs_and_max_decimals() {
        // BTC (sz_decimals = 5 -> max_decimals = 1)
        assert_eq!(format_px(77_415.67, 5).unwrap(), "77416");
        // Oil (sz_decimals = 2 -> max_decimals = 4, 5 sig figs)
        assert_eq!(format_px(102.856, 2).unwrap(), "102.86");
        // Sub-dollar asset (sz_decimals = 0 -> max_decimals = 6, 5 sig figs)
        assert_eq!(format_px(0.1234567, 0).unwrap(), "0.12346");
    }

    #[test]
    fn hip3_asset_id_resolution() {
        assert_eq!(resolve_asset_id(0, 0), 0); // BTC
        assert_eq!(resolve_asset_id(1, 3), 110_003); // HIP-3 dex 1, asset 3
    }

    #[test]
    fn nonce_allocator_is_strictly_monotonic() {
        let alloc = NonceAllocator::new(1_700_000_000_000);
        let n1 = alloc.next_nonce(1_700_000_000_000);
        let n2 = alloc.next_nonce(1_700_000_000_000);
        let n3 = alloc.next_nonce(1_700_000_000_000);
        assert_eq!(n1, 1_700_000_000_001);
        assert_eq!(n2, 1_700_000_000_002);
        assert_eq!(n3, 1_700_000_000_003);
    }

    #[test]
    fn l1_action_signing_produces_valid_recoverable_signature() {
        let signer = PrivateKeySigner::random();
        let action = L1ActionWire::Order {
            orders: vec![OrderWire {
                a: 0,
                b: true,
                p: "77400".to_string(),
                s: "0.01".to_string(),
                r: false,
                t: OrderTypeWire::Limit { tif: WireTif::Ioc },
                c: Some("0x0102030405060708090a0b0c0d0e0f10".to_string()),
            }],
            grouping: OrderGrouping::Na,
            builder: None,
        };
        let sig = sign_l1_action(&signer, &action, 1_730_000_000_000, None, false).unwrap();
        assert!(sig.r.starts_with("0x") && sig.r.len() == 66);
        assert!(sig.s.starts_with("0x") && sig.s.len() == 66);
        assert!(sig.v == 27 || sig.v == 28);
    }

    #[test]
    fn approve_agent_typed_data_hashes_cleanly_via_alloy() {
        let payload = build_approve_agent_typed_data(
            Address::repeat_byte(0x11),
            Some("vaughan-hl"),
            1_730_000_000_000,
            false,
        );
        let td: TypedData = serde_json::from_value(payload).expect("valid EIP-712 JSON");
        let hash = td.eip712_signing_hash().expect("hash succeeds");
        assert_ne!(hash, B256::ZERO);
    }

    #[test]
    fn wire_validation_rejects_malformed_inputs_before_signing() {
        let bad_cloid = L1ActionWire::Order {
            orders: vec![OrderWire {
                a: 0,
                b: true,
                p: "77400".to_string(),
                s: "0.01".to_string(),
                r: false,
                t: OrderTypeWire::Limit { tif: WireTif::Ioc },
                c: Some("0xNOT_A_32_HEX_CLOID".to_string()),
            }],
            grouping: OrderGrouping::Na,
            builder: None,
        };
        assert!(bad_cloid.to_msgpack_bytes().is_err());

        let bad_leverage = L1ActionWire::UpdateLeverage {
            asset: 0,
            is_cross: false,
            leverage: 100,
        };
        assert!(bad_leverage.to_msgpack_bytes().is_err());

        let bad_builder = L1ActionWire::Order {
            orders: vec![OrderWire {
                a: 0,
                b: true,
                p: "77400".to_string(),
                s: "0.01".to_string(),
                r: false,
                t: OrderTypeWire::Limit { tif: WireTif::Ioc },
                c: None,
            }],
            grouping: OrderGrouping::Na,
            builder: Some(BuilderFeeWire {
                b: format!("{:#x}", Address::ZERO),
                f: 10,
            }),
        };
        assert!(bad_builder.to_msgpack_bytes().is_err());
    }
}
