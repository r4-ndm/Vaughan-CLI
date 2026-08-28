//! PulseSwap public quote API (no API key).
//!
//! Docs: <https://docs.pulseswap.io/integrations/api-reference>
//! Prefer `POST /api/v2/quotes/advanced` — standard `/quotes` often returns
//! `amountIn: "0"` incorrectly.

use alloy::primitives::{Address, Bytes, U256};
use serde::Deserialize;
use std::str::FromStr;

use crate::error::WalletError;

use super::catalog::AggVenue;
use super::types::{AggExecTx, AggQuote, AggQuoteRequest};

/// Advanced quote endpoint (more reliable amountIn).
pub const PULSESWAP_QUOTE_URL: &str = "https://quotes.pulseswap.io/api/v2/quotes/advanced";

/// Native PLS sentinel for PulseSwap.
pub const NATIVE_ZERO: &str = "0x0000000000000000000000000000000000000000";

pub struct PulseSwapClient {
    http: reqwest::Client,
    url: String,
}

impl PulseSwapClient {
    pub fn public() -> Result<Self, WalletError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("vaughan-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| WalletError::NetworkError(format!("pulseswap http: {e}")))?;
        Ok(Self {
            http,
            url: PULSESWAP_QUOTE_URL.into(),
        })
    }

    pub async fn quote(&self, req: &AggQuoteRequest) -> Result<AggQuote, WalletError> {
        let from = if req.token_in_is_native {
            NATIVE_ZERO.to_string()
        } else {
            req.token_in.to_string()
        };
        let to = if req.token_out_is_native {
            NATIVE_ZERO.to_string()
        } else {
            req.token_out.to_string()
        };

        let body = serde_json::json!({
            "chainId": 369,
            "platform": "mixed",
            "fromToken": from,
            "toToken": to,
            "amountIn": req.amount_in.to_string(),
            "slippage": req.slippage_percent,
            "userAddress": req.account.map(|a| format!("{a:#x}")),
            // PulseSwap returns amountIn/Out 0 and no tx without pricing hints
            // (see docs.pulseswap.io — extra.tokenInPrice / tokenOutPrice / gasPrice).
            "extra": {
                "tokenInPrice": 1.0,
                "tokenOutPrice": 1.0,
                "gasPrice": "1000000000",
                "gasTokenPrice": 1.0,
            },
        });

        let resp = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| WalletError::NetworkError(format!("pulseswap quote: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| WalletError::NetworkError(format!("pulseswap body: {e}")))?;

        if !status.is_success() {
            let snippet: String = text.chars().take(180).collect();
            return Err(WalletError::NetworkError(format!(
                "pulseswap HTTP {status}: {snippet}"
            )));
        }

        let envelope: PulseEnvelope = serde_json::from_str(&text)
            .map_err(|e| WalletError::Serialization(format!("pulseswap JSON: {e}")))?;
        if !envelope.success {
            return Err(WalletError::NetworkError(format!(
                "pulseswap: {}",
                envelope.message.unwrap_or_else(|| "quote failed".into())
            )));
        }
        let data = envelope
            .data
            .ok_or_else(|| WalletError::NetworkError("pulseswap: empty data".into()))?;
        if !data.success {
            return Err(WalletError::NetworkError(
                "pulseswap: no route for this pair/amount".into(),
            ));
        }

        let amount_in = parse_u256(&data.amount_in)?;
        let amount_out = parse_u256(&data.amount_out)?;
        if amount_out.is_zero() {
            return Err(WalletError::NetworkError(
                "pulseswap: quote returned zero amountOut".into(),
            ));
        }
        let tx = data
            .tx
            .ok_or_else(|| WalletError::NetworkError("pulseswap: missing tx calldata".into()))?;
        let to_addr = Address::from_str(tx.to.trim())
            .map_err(|_| WalletError::InvalidTransaction("pulseswap tx.to invalid".into()))?;
        let mut value = parse_u256(&tx.value)?;
        // API often returns value=0 for native PLS — patch from request amount.
        if req.token_in_is_native && value.is_zero() {
            value = req.amount_in;
        }
        // Prefer request amount if API wrongly returns amountIn 0.
        let amount_in = if amount_in.is_zero() {
            req.amount_in
        } else {
            amount_in
        };

        super::routers::assert_agg_exec_targets(to_addr, to_addr)?;

        Ok(AggQuote {
            venue: AggVenue::PulseSwap,
            amount_in,
            amount_out,
            gas_estimate: data.gas_estimate,
            tx: AggExecTx {
                to: to_addr,
                data: parse_bytes(&tx.data)?,
                value,
            },
            spender: to_addr,
        })
    }
}

#[derive(Debug, Deserialize)]
struct PulseEnvelope {
    success: bool,
    data: Option<PulseData>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PulseData {
    success: bool,
    amount_in: String,
    amount_out: String,
    #[serde(default)]
    gas_estimate: Option<u64>,
    tx: Option<PulseTx>,
}

#[derive(Debug, Deserialize)]
struct PulseTx {
    to: String,
    data: String,
    value: String,
}

fn parse_u256(s: &str) -> Result<U256, WalletError> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(U256::ZERO);
    }
    U256::from_str(t).map_err(|_| WalletError::InvalidAmount(format!("pulseswap amount: {t}")))
}

fn parse_bytes(s: &str) -> Result<Bytes, WalletError> {
    let hex = s.trim().trim_start_matches("0x");
    let bytes = hex::decode(hex)
        .map_err(|e| WalletError::InvalidTransaction(format!("pulseswap calldata: {e}")))?;
    Ok(Bytes::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn parses_advanced_fixture() {
        let raw = r#"{
          "success":true,
          "data":{
            "success":true,
            "amountIn":"1000000000000000000",
            "amountOut":"14376543298151",
            "gasEstimate":163601,
            "tx":{
              "to":"0xC994375187988C751C8fCb96A68A0f242947f0E6",
              "data":"0x2d09aed5",
              "value":"0"
            }
          },
          "message":"OK"
        }"#;
        let env: PulseEnvelope = serde_json::from_str(raw).unwrap();
        let d = env.data.unwrap();
        assert!(d.success);
        assert_eq!(d.amount_in, "1000000000000000000");
        assert_eq!(
            Address::from_str(&d.tx.unwrap().to).unwrap(),
            address!("0xC994375187988C751C8fCb96A68A0f242947f0E6")
        );
    }

    #[test]
    fn rejects_inner_success_false() {
        let raw = r#"{
          "success":true,
          "data":{
            "success":false,
            "amountIn":"0",
            "amountOut":"0",
            "gasEstimate":0,
            "tx":null
          },
          "message":"OK"
        }"#;
        let env: PulseEnvelope = serde_json::from_str(raw).unwrap();
        let d = env.data.unwrap();
        assert!(!d.success);
    }

    /// Live PulseChain quote — PLS → M3M3 (same path as Empseal compare tests).
    #[tokio::test]
    #[ignore = "live PulseSwap API"]
    async fn live_pulseswap_pls_to_m3m3() {
        use super::super::types::AggQuoteRequest;
        use alloy::primitives::address;

        let client = PulseSwapClient::public().unwrap();
        let m3m3 = address!("0x78a2809e8e2ef8e07429559f15703ee20e885588");
        let account: Address = "0x742d35Cc6634C0532925a3b8D221691B5c1b6b29"
            .parse()
            .unwrap();
        let req = AggQuoteRequest {
            token_in: Address::ZERO,
            token_out: m3m3,
            token_in_is_native: true,
            token_out_is_native: false,
            amount_in: U256::from(1_000_000_u64) * U256::from(10u64).pow(U256::from(24)),
            slippage_percent: 0.5,
            account: Some(account),
        };
        let q = client.quote(&req).await.expect("pulseswap live quote");
        assert!(q.amount_out > U256::ZERO);
        assert!(!q.tx.data.is_empty());
        assert_eq!(
            q.tx.to,
            address!("0xC994375187988C751C8fCb96A68A0f242947f0E6")
        );
    }
}
