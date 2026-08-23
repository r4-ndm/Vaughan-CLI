//! Request / response shapes for the Piteas quote API.

use alloy::primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::error::WalletError;

/// Mainnet PiteasRouter — send `methodParameters.calldata` (+ value) here.
///
/// Source: <https://docs.piteas.io/contracts>
pub const PITEAS_ROUTER_MAINNET: &str = "0x6BF228eb7F8ad948d37deD07E595EfddfaAF88A6";

/// Sentinel the quote API accepts for native PLS (not WPLS).
pub const NATIVE_PLS: &str = "PLS";

/// How to spell the input / output asset for a quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToken {
    /// Native PLS — API string `"PLS"`.
    Pls,
    /// ERC-20 (or WPLS) contract address.
    Address(Address),
}

impl NativeToken {
    /// Encode for the `tokenInAddress` / `tokenOutAddress` query param.
    pub fn as_query(&self) -> String {
        self.to_string()
    }
}

impl std::fmt::Display for NativeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pls => write!(f, "{NATIVE_PLS}"),
            Self::Address(a) => write!(f, "{a}"),
        }
    }
}

/// Parameters for `GET …/quote`.
#[derive(Debug, Clone)]
pub struct QuoteRequest {
    pub token_in: NativeToken,
    pub token_out: NativeToken,
    /// Amount in smallest units (token decimals already applied).
    pub amount: U256,
    /// Slippage percent (API default 0.5).
    pub allowed_slippage: f64,
    /// Optional receiver; omitted → msg.sender on-chain.
    pub account: Option<Address>,
}

impl QuoteRequest {
    pub fn new(token_in: NativeToken, token_out: NativeToken, amount: U256) -> Self {
        Self {
            token_in,
            token_out,
            amount,
            allowed_slippage: 0.5,
            account: None,
        }
    }

    pub fn with_slippage(mut self, pct: f64) -> Self {
        self.allowed_slippage = pct;
        self
    }

    pub fn with_account(mut self, account: Address) -> Self {
        self.account = Some(account);
        self
    }
}

/// Token metadata in a quote response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiteasToken {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
    pub chain_id: u64,
}

/// Calldata + native value for the PiteasRouter transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodParameters {
    /// Hex calldata (`0x…`).
    pub calldata: String,
    /// Hex wei value (`0x…`) — typically non-zero for native PLS in.
    pub value: String,
}

impl MethodParameters {
    /// Decode calldata bytes for a wallet tx.
    pub fn calldata_bytes(&self) -> Result<Bytes, WalletError> {
        parse_hex_bytes(&self.calldata)
    }

    /// Decode native value as [`U256`].
    pub fn value_u256(&self) -> Result<U256, WalletError> {
        parse_u256_hex(&self.value)
    }

    /// PiteasRouter target on PulseChain mainnet.
    pub fn router_address() -> Result<Address, WalletError> {
        Address::from_str(PITEAS_ROUTER_MAINNET)
            .map_err(|_| WalletError::InvalidTransaction("PiteasRouter address is invalid".into()))
    }
}

/// Successful quote payload from Pathfinder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiteasQuote {
    pub src_token: PiteasToken,
    pub dest_token: PiteasToken,
    /// Hex amount in.
    pub src_amount: String,
    /// Hex amount out (expected).
    pub dest_amount: String,
    pub gas_use_estimate: u64,
    #[serde(default)]
    pub gas_use_estimate_usd: Option<f64>,
    pub method_parameters: MethodParameters,
    /// Opaque route graph — keep for diagnostics; not required to execute.
    #[serde(default)]
    pub route: Option<serde_json::Value>,
}

impl PiteasQuote {
    pub fn dest_amount_u256(&self) -> Result<U256, WalletError> {
        parse_u256_hex(&self.dest_amount)
    }

    pub fn src_amount_u256(&self) -> Result<U256, WalletError> {
        parse_u256_hex(&self.src_amount)
    }
}

fn parse_hex_bytes(s: &str) -> Result<Bytes, WalletError> {
    let hex = s.trim().trim_start_matches("0x");
    let bytes = hex::decode(hex)
        .map_err(|e| WalletError::InvalidTransaction(format!("piteas calldata hex: {e}")))?;
    Ok(Bytes::from(bytes))
}

fn parse_u256_hex(s: &str) -> Result<U256, WalletError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(WalletError::InvalidAmount("empty piteas amount".into()));
    }
    U256::from_str(t).map_err(|_| {
        WalletError::InvalidAmount(format!("piteas amount is not a valid U256 hex: {t}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_pls_query_spelling() {
        assert_eq!(NativeToken::Pls.to_string(), "PLS");
        assert_eq!(NativeToken::Pls.as_query(), "PLS");
    }

    #[test]
    fn parse_fixture_quote() {
        let raw = r#"{
          "srcToken":{"address":"0xA1077a294dDE1B09bB078844df40758a5D0f9a27","symbol":"WPLS","decimals":18,"chainId":369},
          "destToken":{"address":"0xefD766cCb38EaF1dfd701853BFCe31359239F305","symbol":"DAI","decimals":18,"chainId":369},
          "srcAmount":"0xde0b6b3a7640000",
          "destAmount":"0xdb8f5d6716d",
          "gasUseEstimate":760000,
          "gasUseEstimateUSD":0.005,
          "methodParameters":{"calldata":"0x8218b58f","value":"0xde0b6b3a7640000"},
          "route":{"paths":[]}
        }"#;
        let q: PiteasQuote = serde_json::from_str(raw).unwrap();
        assert_eq!(q.src_token.symbol, "WPLS");
        assert_eq!(q.dest_token.symbol, "DAI");
        assert_eq!(q.method_parameters.calldata, "0x8218b58f");
        assert_eq!(
            q.src_amount_u256().unwrap(),
            U256::from(1_000_000_000_000_000_000u64)
        );
        assert!(!q.method_parameters.calldata_bytes().unwrap().is_empty());
    }
}
