//! SquirrelSwap Brain API — public aggregator, **no API key**.
//!
//! Base: `https://api.squirrelswap.pro` (override with env in their MCP;
//! Vaughan pins the public host). Attribution header `X-SS-Client` only —
//! not authentication.
//!
//! - `GET /quote` — slim preview (amount out, route string)
//! - `POST /swap` — `{tx:{to,data,value}}` + optional `approvalNeeded`
//!
//! Native PLS: `tokenIn` / `tokenOut` = `0x000…000`.
//! Source: `squirrelswap-mcp` (`brain.js` / `tools.js`).

use alloy::primitives::{Address, Bytes, U256};
use serde::Deserialize;
use std::str::FromStr;

use crate::error::WalletError;

use super::catalog::AggVenue;
use super::types::{AggExecTx, AggQuote, AggQuoteRequest};

/// Production Brain origin.
pub const SQUIRRELSWAP_BRAIN_URL: &str = "https://api.squirrelswap.pro";

/// Native PLS sentinel for Brain.
pub const NATIVE_ZERO: &str = "0x0000000000000000000000000000000000000000";

/// Vaughan attribution (not a secret).
const CLIENT_HEADER: &str = concat!("vaughan-cli/", env!("CARGO_PKG_VERSION"));

pub struct SquirrelSwapClient {
    http: reqwest::Client,
    base: String,
}

impl SquirrelSwapClient {
    pub fn public() -> Result<Self, WalletError> {
        let http = reqwest::Client::builder()
            .user_agent(CLIENT_HEADER)
            .build()
            .map_err(|e| WalletError::NetworkError(format!("squirrelswap http: {e}")))?;
        Ok(Self {
            http,
            base: SQUIRRELSWAP_BRAIN_URL.trim_end_matches('/').to_string(),
        })
    }

    /// Prepare a signable swap (`POST /swap`). Requires `req.account` (recipient).
    pub async fn prepare_swap(&self, req: &AggQuoteRequest) -> Result<AggQuote, WalletError> {
        let recipient = req.account.ok_or_else(|| {
            WalletError::InvalidTransaction(
                "squirrelswap needs recipient (active wallet address)".into(),
            )
        })?;

        let token_in = if req.token_in_is_native {
            NATIVE_ZERO.to_string()
        } else {
            format!("{:#x}", req.token_in)
        };
        let token_out = if req.token_out_is_native {
            NATIVE_ZERO.to_string()
        } else {
            format!("{:#x}", req.token_out)
        };

        let body = serde_json::json!({
            "tokenIn": token_in,
            "tokenOut": token_out,
            "amountIn": req.amount_in.to_string(),
            "slippage": req.slippage_percent,
            "recipient": format!("{recipient:#x}"),
        });

        let resp = self
            .http
            .post(format!("{}/swap", self.base))
            .header("X-SS-Client", CLIENT_HEADER)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| WalletError::NetworkError(format!("squirrelswap /swap: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| WalletError::NetworkError(format!("squirrelswap body: {e}")))?;

        if status.as_u16() == 429 {
            return Err(WalletError::NetworkError(
                "squirrelswap rate limited (429) — slow down and retry".into(),
            ));
        }
        if !status.is_success() {
            let snippet: String = text.chars().take(180).collect();
            return Err(WalletError::NetworkError(format!(
                "squirrelswap HTTP {status}: {snippet}"
            )));
        }

        let parsed: SwapResponse = serde_json::from_str(&text)
            .map_err(|e| WalletError::Serialization(format!("squirrelswap /swap JSON: {e}")))?;
        if !parsed.success {
            return Err(WalletError::NetworkError(format!(
                "squirrelswap: {}",
                parsed.error.unwrap_or_else(|| "swap prepare failed".into())
            )));
        }
        let tx = parsed
            .tx
            .ok_or_else(|| WalletError::NetworkError("squirrelswap: missing tx".into()))?;
        let quote = parsed.quote.unwrap_or_default();
        let to = Address::from_str(tx.to.trim())
            .map_err(|_| WalletError::InvalidTransaction("squirrelswap tx.to invalid".into()))?;
        let amount_out = parse_u256(
            quote
                .amount_out
                .as_deref()
                .or(quote.net_amount_out.as_deref())
                .unwrap_or("0"),
        )?;
        let spender = parsed
            .approval_needed
            .as_ref()
            .and_then(|a| Address::from_str(a.spender.trim()).ok())
            .unwrap_or(to);

        Ok(AggQuote {
            venue: AggVenue::SquirrelSwap,
            amount_in: req.amount_in,
            amount_out,
            gas_estimate: None,
            tx: AggExecTx {
                to,
                data: parse_bytes(&tx.data)?,
                value: parse_u256(&tx.value)?,
            },
            spender,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwapResponse {
    success: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    tx: Option<SwapTx>,
    #[serde(default)]
    quote: Option<SwapQuote>,
    #[serde(default)]
    approval_needed: Option<ApprovalNeeded>,
}

#[derive(Debug, Deserialize)]
struct SwapTx {
    to: String,
    data: String,
    value: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwapQuote {
    #[serde(default)]
    amount_out: Option<String>,
    #[serde(default)]
    net_amount_out: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApprovalNeeded {
    spender: String,
}

fn parse_u256(s: &str) -> Result<U256, WalletError> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(U256::ZERO);
    }
    U256::from_str(t).map_err(|_| WalletError::InvalidAmount(format!("squirrelswap amount: {t}")))
}

fn parse_bytes(s: &str) -> Result<Bytes, WalletError> {
    let hex = s.trim().trim_start_matches("0x");
    let bytes = hex::decode(hex)
        .map_err(|e| WalletError::InvalidTransaction(format!("squirrelswap calldata: {e}")))?;
    Ok(Bytes::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_swap_fixture() {
        let raw = r#"{
          "success": true,
          "tx": {
            "to": "0xDa8953Fc615d6E816b9647Afd5536123dcE70B78",
            "data": "0xc563dcec",
            "value": "1003000000000000000"
          },
          "quote": { "amountOut": "15683509506112", "netAmountOut": "15683509506112" },
          "approvalNeeded": {
            "token": "0xa1077a294dde1b09bb078844df40758a5d0f9a27",
            "spender": "0xDa8953Fc615d6E816b9647Afd5536123dcE70B78",
            "amount": "1000000000000000000"
          }
        }"#;
        let p: SwapResponse = serde_json::from_str(raw).unwrap();
        assert!(p.success);
        assert_eq!(
            p.tx.unwrap().to,
            "0xDa8953Fc615d6E816b9647Afd5536123dcE70B78"
        );
        assert_eq!(
            p.quote.unwrap().amount_out.as_deref(),
            Some("15683509506112")
        );
    }
}
