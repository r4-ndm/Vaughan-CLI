//! Request / response shapes for LibertySwap `v3/swap/quote`.

use alloy::primitives::{Address, Bytes, U256};
use serde::Deserialize;
use std::str::FromStr;

use crate::error::WalletError;

/// Asset symbol / address spelling for the quote API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeAsset {
    /// Stable / named asset (`USDC`, `ETH`, …).
    Symbol(&'static str),
    /// Explicit contract address.
    Address(Address),
}

impl BridgeAsset {
    pub fn as_query(&self) -> String {
        match self {
            Self::Symbol(s) => (*s).to_string(),
            Self::Address(a) => format!("{a:#x}"),
        }
    }
}

/// Preset EVM leg for the Bridge TUI (Pulse-centered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeChainPreset {
    pub chain_id: u64,
    pub label: &'static str,
    /// Default token symbol for this chain in the USDC bridge flow.
    pub default_token: &'static str,
}

/// Pulse ↔ major EVM presets (USDC-first).
pub const BRIDGE_CHAIN_PRESETS: &[BridgeChainPreset] = &[
    BridgeChainPreset {
        chain_id: 369,
        label: "PulseChain",
        default_token: "USDC",
    },
    BridgeChainPreset {
        chain_id: 8453,
        label: "Base",
        default_token: "USDC",
    },
    BridgeChainPreset {
        chain_id: 1,
        label: "Ethereum",
        default_token: "USDC",
    },
    BridgeChainPreset {
        chain_id: 56,
        label: "BSC",
        default_token: "USDC",
    },
    BridgeChainPreset {
        chain_id: 42161,
        label: "Arbitrum",
        default_token: "USDC",
    },
    BridgeChainPreset {
        chain_id: 137,
        label: "Polygon",
        default_token: "USDC",
    },
];

/// Quote input for LibertySwap.
#[derive(Debug, Clone)]
pub struct BridgeQuoteRequest {
    pub src_token: BridgeAsset,
    pub dst_token: BridgeAsset,
    pub amount: U256,
    pub src_chain: u64,
    pub dst_chain: u64,
    /// Required by live `v3/swap/quote`.
    pub recipient: Address,
}

/// Token metadata in a quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeTokenInfo {
    pub address: Address,
    pub symbol: String,
    pub decimals: u8,
    pub chain_id: u64,
}

/// Protocol fee summary.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeFee {
    pub percentage: f64,
    pub amount: U256,
}

/// Optional ERC-20 approve step from the quote.
#[derive(Debug, Clone)]
pub struct BridgeApproval {
    pub token: Address,
    pub spender: Address,
    pub amount: U256,
}

/// Calldata ready for the source-chain wallet tx.
#[derive(Debug, Clone)]
pub struct BridgeExecTx {
    pub to: Address,
    pub data: Bytes,
    pub value: U256,
}

/// Normalized LibertySwap quote (source broadcast only in Vaughan v1).
#[derive(Debug, Clone)]
pub struct BridgeQuote {
    pub to: Address,
    pub src_token: BridgeTokenInfo,
    pub dest_token: BridgeTokenInfo,
    pub src_amount: U256,
    pub dest_amount: U256,
    pub fee: BridgeFee,
    pub approval: Option<BridgeApproval>,
    pub tx: BridgeExecTx,
}

// ── Wire types (serde) ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireQuote {
    pub to: String,
    pub src_token: WireToken,
    pub dest_token: WireToken,
    pub src_amount: String,
    pub dest_amount: String,
    pub fee: WireFee,
    #[serde(default)]
    pub approval: Option<WireApproval>,
    pub method_parameters: WireMethodParameters,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireToken {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
    pub chain_id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireFee {
    pub protocol: WireFeePart,
    pub total: WireFeeTotal,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireFeePart {
    pub percentage: f64,
    #[serde(rename = "amount")]
    pub _amount: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireFeeTotal {
    pub amount: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireApproval {
    pub token: String,
    pub spender: String,
    pub amount: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireMethodParameters {
    pub calldata: String,
    pub value: String,
}

impl WireQuote {
    pub(crate) fn into_bridge_quote(self) -> Result<BridgeQuote, WalletError> {
        let to = parse_address(&self.to, "liberty to")?;
        let approval = match self.approval {
            Some(a) => Some(BridgeApproval {
                token: parse_address(&a.token, "liberty approval.token")?,
                spender: parse_address(&a.spender, "liberty approval.spender")?,
                amount: parse_u256_dec(&a.amount)?,
            }),
            None => None,
        };
        Ok(BridgeQuote {
            to,
            src_token: BridgeTokenInfo {
                address: parse_address(&self.src_token.address, "liberty srcToken")?,
                symbol: self.src_token.symbol,
                decimals: self.src_token.decimals,
                chain_id: self.src_token.chain_id,
            },
            dest_token: BridgeTokenInfo {
                address: parse_address(&self.dest_token.address, "liberty destToken")?,
                symbol: self.dest_token.symbol,
                decimals: self.dest_token.decimals,
                chain_id: self.dest_token.chain_id,
            },
            src_amount: parse_u256_dec(&self.src_amount)?,
            dest_amount: parse_u256_dec(&self.dest_amount)?,
            fee: BridgeFee {
                percentage: self.fee.protocol.percentage,
                amount: parse_u256_dec(&self.fee.total.amount)?,
            },
            approval,
            tx: BridgeExecTx {
                to,
                data: parse_hex_bytes(&self.method_parameters.calldata)?,
                value: parse_u256_hex(&self.method_parameters.value)?,
            },
        })
    }
}

pub(crate) fn parse_address(s: &str, ctx: &str) -> Result<Address, WalletError> {
    Address::from_str(s.trim())
        .map_err(|_| WalletError::InvalidTransaction(format!("{ctx}: bad address")))
}

pub(crate) fn parse_u256_dec(s: &str) -> Result<U256, WalletError> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(U256::ZERO);
    }
    U256::from_str(t).map_err(|_| WalletError::InvalidAmount(format!("liberty amount: {t}")))
}

pub(crate) fn parse_u256_hex(s: &str) -> Result<U256, WalletError> {
    let t = s.trim();
    if t.is_empty() || t == "0x" || t == "0x0" || t == "0x00" {
        return Ok(U256::ZERO);
    }
    let hex = t.trim_start_matches("0x");
    U256::from_str_radix(hex, 16)
        .map_err(|_| WalletError::InvalidAmount(format!("liberty value hex: {t}")))
}

pub(crate) fn parse_hex_bytes(s: &str) -> Result<Bytes, WalletError> {
    let hex = s.trim().trim_start_matches("0x");
    let bytes = hex::decode(hex)
        .map_err(|e| WalletError::InvalidTransaction(format!("liberty calldata: {e}")))?;
    Ok(Bytes::from(bytes))
}
