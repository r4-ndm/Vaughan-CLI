//! 9mm 9X unified API — public swap quotes at `api.9mm.pro` (no API key).
//!
//! Docs: <https://api.9mm.pro/docs> (OpenAPI at `/openapi.yaml`).
//! Proxies the 9x `/swap/v1/*` surface under `/v1/{chain}/swap/*`.
//!
//! - `GET /v1/{chain}/swap/price` — indicative quote (no calldata)
//! - `GET /v1/{chain}/swap/quote` — executable calldata (`to`, `data`, `value`)
//!
//! Native PLS uses the 0x-style sentinel `0xEeee…EeeE` (same family as Switch).
//! Cloudflare rejects bare library User-Agents — always send our own.

use alloy::primitives::{Address, Bytes, U256};
use serde::Deserialize;
use serde::de::{self, Deserializer};
use std::str::FromStr;

use crate::error::WalletError;

use super::catalog::AggVenue;
use super::types::{AggExecTx, AggQuote, AggQuoteRequest};

/// Production unified API origin.
pub const NINEMM_API_URL: &str = "https://api.9mm.pro";

/// Native token sentinel for 9x / 0x-style swap APIs on EVM chains.
pub const NATIVE_EEEE: &str = "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE";

const CLIENT_HEADER: &str = concat!("vaughan-cli/", env!("CARGO_PKG_VERSION"));

pub struct NineMmClient {
    http: reqwest::Client,
    base: String,
    chain_slug: &'static str,
}

impl NineMmClient {
    pub fn for_chain(chain_id: u64) -> Result<Self, WalletError> {
        let chain_slug = chain_slug(chain_id)?;
        let http = reqwest::Client::builder()
            .user_agent(CLIENT_HEADER)
            .build()
            .map_err(|e| WalletError::NetworkError(format!("9mm http: {e}")))?;
        Ok(Self {
            http,
            base: NINEMM_API_URL.trim_end_matches('/').to_string(),
            chain_slug,
        })
    }

    /// Indicative price (`/swap/price`) — no wallet required.
    pub async fn preview_price(&self, req: &AggQuoteRequest) -> Result<NineMmPreview, WalletError> {
        let url = self.build_url("price", req, None)?;
        let parsed: SwapPriceResponse = self.get_json(&url).await?;
        let amount_out = parse_u256(&parsed.buy_amount)?;
        if amount_out.is_zero() {
            return Err(WalletError::NetworkError(
                "9mm: zero buyAmount on price preview".into(),
            ));
        }
        Ok(NineMmPreview {
            amount_out,
            gas_estimate: parsed.estimated_gas,
            price_impact: parsed.estimated_price_impact,
            sources: parsed
                .sources
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.proportion.as_deref() != Some("0"))
                .map(|s| s.name)
                .collect(),
        })
    }

    /// Map `/swap/price` into [`AggQuote`] for compare-only ranking (no calldata).
    pub fn preview_to_agg_quote(req: &AggQuoteRequest, preview: NineMmPreview) -> AggQuote {
        AggQuote {
            venue: AggVenue::NineMm9x,
            amount_in: req.amount_in,
            amount_out: preview.amount_out,
            gas_estimate: preview.gas_estimate,
            tx: AggExecTx {
                to: Address::ZERO,
                data: Bytes::new(),
                value: U256::ZERO,
            },
            spender: Address::ZERO,
            preview_only: true,
        }
    }

    /// Executable quote (`/swap/quote`). Requires `req.account` (gas simulation).
    pub async fn quote(&self, req: &AggQuoteRequest) -> Result<AggQuote, WalletError> {
        let taker = req.account.ok_or_else(|| {
            WalletError::InvalidTransaction("9mm needs takerAddress (active wallet)".into())
        })?;
        let url = self.build_url("quote", req, Some(taker))?;
        let parsed: SwapQuoteResponse = self.get_json(&url).await?;

        let to = Address::from_str(parsed.to.trim())
            .map_err(|_| WalletError::InvalidTransaction("9mm quote.to invalid".into()))?;
        let amount_out = parse_u256(&parsed.buy_amount)?;
        if amount_out.is_zero() {
            return Err(WalletError::NetworkError(
                "9mm: zero buyAmount on quote".into(),
            ));
        }

        let mut value = parse_u256(parsed.value.as_deref().unwrap_or("0"))?;
        if req.token_in_is_native && value.is_zero() {
            value = req.amount_in;
        }

        let spender = parsed
            .allowance_target
            .as_deref()
            .and_then(|s| Address::from_str(s.trim()).ok())
            .filter(|a| *a != Address::ZERO)
            .unwrap_or(to);

        super::routers::assert_agg_exec_targets(to, spender)?;

        Ok(AggQuote {
            venue: AggVenue::NineMm9x,
            amount_in: req.amount_in,
            amount_out,
            gas_estimate: parsed.estimated_gas,
            tx: AggExecTx {
                to,
                data: parse_bytes(parsed.data.as_deref().unwrap_or("0x"))?,
                value,
            },
            spender,
            preview_only: false,
        })
    }

    fn build_url(
        &self,
        endpoint: &str,
        req: &AggQuoteRequest,
        taker: Option<Address>,
    ) -> Result<String, WalletError> {
        let sell = token_param(req.token_in, req.token_in_is_native);
        let buy = token_param(req.token_out, req.token_out_is_native);
        let mut url = format!(
            "{}/v1/{}/swap/{endpoint}?sellToken={sell}&buyToken={buy}&sellAmount={}",
            self.base, self.chain_slug, req.amount_in
        );
        if endpoint == "quote" {
            let taker = taker.ok_or_else(|| {
                WalletError::InvalidTransaction("9mm quote needs takerAddress".into())
            })?;
            url.push_str(&format!(
                "&takerAddress={taker:#x}&slippagePercentage={}",
                req.slippage_percent
            ));
        }
        Ok(url)
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T, WalletError> {
        let resp = self
            .http
            .get(url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| WalletError::NetworkError(format!("9mm GET: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| WalletError::NetworkError(format!("9mm body: {e}")))?;

        if status.as_u16() == 429 {
            return Err(WalletError::NetworkError(
                "9mm rate limited (429) — retry or request a free API key".into(),
            ));
        }
        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<NineMmError>(&text) {
                let detail = err
                    .reason
                    .or(err.message)
                    .unwrap_or_else(|| format!("HTTP {status}"));
                let msg = match err.code {
                    Some(c) => format!("9mm [{c}]: {detail}"),
                    None => format!("9mm: {detail}"),
                };
                return Err(WalletError::NetworkError(msg));
            }
            let snippet: String = text.chars().take(180).collect();
            return Err(WalletError::NetworkError(format!(
                "9mm HTTP {status}: {snippet}"
            )));
        }

        serde_json::from_str(&text)
            .map_err(|e| WalletError::NetworkError(format!("9mm JSON: {e}")))
    }
}

/// Slim preview from `/swap/price`.
#[derive(Debug, Clone)]
pub struct NineMmPreview {
    pub amount_out: U256,
    pub gas_estimate: Option<u64>,
    pub price_impact: Option<String>,
    pub sources: Vec<String>,
}

fn chain_slug(chain_id: u64) -> Result<&'static str, WalletError> {
    match chain_id {
        369 => Ok("pulse"),
        1 => Ok("eth"),
        8453 => Ok("base"),
        146 => Ok("sonic"),
        4663 => Ok("robinhood"),
        _ => Err(WalletError::Other(format!(
            "9mm API: unsupported chain_id {chain_id}"
        ))),
    }
}

fn token_param(addr: Address, is_native: bool) -> String {
    if is_native {
        NATIVE_EEEE.to_string()
    } else {
        format!("{addr:#x}")
    }
}

#[derive(Debug, Deserialize)]
struct SwapPriceResponse {
    #[serde(rename = "buyAmount")]
    buy_amount: String,
    #[serde(default, rename = "estimatedGas", deserialize_with = "de_opt_u64")]
    estimated_gas: Option<u64>,
    #[serde(default, rename = "estimatedPriceImpact")]
    estimated_price_impact: Option<String>,
    #[serde(default)]
    sources: Option<Vec<SourceSplit>>,
}

#[derive(Debug, Deserialize)]
struct SwapQuoteResponse {
    to: String,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(rename = "buyAmount")]
    buy_amount: String,
    #[serde(default, rename = "allowanceTarget")]
    allowance_target: Option<String>,
    #[serde(default, rename = "estimatedGas", deserialize_with = "de_opt_u64")]
    estimated_gas: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SourceSplit {
    name: String,
    #[serde(default)]
    proportion: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NineMmError {
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<i64>,
}

fn parse_u256(s: &str) -> Result<U256, WalletError> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(U256::ZERO);
    }
    U256::from_str(t).map_err(|_| WalletError::InvalidAmount(format!("9mm amount: {t}")))
}

fn parse_bytes(s: &str) -> Result<Bytes, WalletError> {
    let hex = s.trim().trim_start_matches("0x");
    if hex.is_empty() {
        return Ok(Bytes::new());
    }
    let bytes = hex::decode(hex)
        .map_err(|e| WalletError::InvalidTransaction(format!("9mm calldata: {e}")))?;
    Ok(Bytes::from(bytes))
}

/// 9mm API returns some numeric fields as JSON strings or numbers depending on route.
fn de_opt_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .ok_or_else(|| de::Error::custom("9mm: estimatedGas number out of range"))
            .map(Some),
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<u64>()
                    .map(Some)
                    .map_err(|_| de::Error::custom(format!("9mm: invalid estimatedGas `{t}`")))
            }
        }
        _ => Err(de::Error::custom("9mm: estimatedGas must be a number or string")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn chain_slug_pulse() {
        assert_eq!(chain_slug(369).unwrap(), "pulse");
    }

    #[test]
    fn native_token_uses_eeee_sentinel() {
        let req = AggQuoteRequest {
            token_in: Address::ZERO,
            token_out: address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39"),
            token_in_is_native: true,
            token_out_is_native: false,
            amount_in: U256::from(1_000_000_000_000_000_000u64),
            slippage_percent: 0.5,
            account: None,
        };
        let client = NineMmClient::for_chain(369).unwrap();
        let url = client.build_url("price", &req, None).unwrap();
        assert!(url.contains("sellToken=0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE"));
        assert!(url.contains("sellAmount=1000000000000000000"));
    }

    #[test]
    fn parses_price_fixture() {
        let raw = r#"{
          "chainId": 369,
          "buyAmount": "2870748",
          "estimatedGas": 535929,
          "estimatedPriceImpact": "0",
          "sellAmount": "1000000000000000000",
          "sources": [{"name":"PulseX_V1","proportion":"1"}]
        }"#;
        let p: SwapPriceResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(p.buy_amount, "2870748");
        assert_eq!(p.estimated_gas, Some(535929));
    }

    #[test]
    fn parses_price_fixture_string_estimated_gas() {
        let raw = r#"{
          "chainId": 369,
          "buyAmount": "4245003433409448080010",
          "estimatedGas": "451812",
          "estimatedPriceImpact": "0",
          "sellAmount": "1000000000000000000",
          "sources": [{"name":"MultiHop","proportion":"1","hops":["NineMM_V3","PulseX_V2"]}]
        }"#;
        let p: SwapPriceResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(p.buy_amount, "4245003433409448080010");
        assert_eq!(p.estimated_gas, Some(451_812));
    }

    #[test]
    fn preview_maps_to_compare_quote_without_calldata() {
        let req = AggQuoteRequest {
            token_in: Address::ZERO,
            token_out: address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39"),
            token_in_is_native: true,
            token_out_is_native: false,
            amount_in: U256::from(1_000_000_000_000_000_000u64),
            slippage_percent: 0.5,
            account: None,
        };
        let preview = NineMmPreview {
            amount_out: U256::from(2_870_748u64),
            gas_estimate: Some(535_929),
            price_impact: Some("0".into()),
            sources: vec!["MultiHop".into()],
        };
        let q = NineMmClient::preview_to_agg_quote(&req, preview);
        assert!(q.preview_only);
        assert!(!q.is_executable());
        assert!(q.tx.data.is_empty());
        assert_eq!(q.amount_out, U256::from(2_870_748u64));
        assert!(
            super::super::routers::assert_agg_exec_targets(q.tx.to, q.spender).is_err(),
            "preview quotes must not pass the exec allowlist"
        );
    }

    #[test]
    fn parses_quote_fixture() {
        let raw = r#"{
          "chainId": 369,
          "to": "0xd5b775d1f15a864a5f6b94624e049cf758d013f7",
          "data": "0x415565b0",
          "value": "1000000000000000000",
          "buyAmount": "2870748",
          "allowanceTarget": "0x0000000000000000000000000000000000000000",
          "estimatedGas": 535929
        }"#;
        let q: SwapQuoteResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            q.to.to_lowercase(),
            "0xd5b775d1f15a864a5f6b94624e049cf758d013f7"
        );
        assert_eq!(q.buy_amount, "2870748");
    }

    #[test]
    fn quote_url_includes_taker_and_slippage() {
        let taker = address!("0x14a54e673e626e25d8d8719005aec8c0992385e2");
        let req = AggQuoteRequest {
            token_in: Address::ZERO,
            token_out: address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39"),
            token_in_is_native: true,
            token_out_is_native: false,
            amount_in: U256::from(1_000_000_000_000_000_000u64),
            slippage_percent: 0.5,
            account: Some(taker),
        };
        let client = NineMmClient::for_chain(369).unwrap();
        let url = client.build_url("quote", &req, Some(taker)).unwrap();
        assert!(url.contains("/swap/quote"));
        assert!(url.contains("takerAddress="));
        assert!(url.contains("slippagePercentage=0.5"));
    }
}

/// Live mainnet probes — no broadcast.
#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::core::format_base_units;
    use alloy::primitives::address;

    const HEX: Address = address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39");
    const USDC: Address = address!("0x15D38573d2feeb82e7ad5187aB8c1D52810B1f07");

    #[tokio::test]
    #[ignore = "hits api.9mm.pro — run with --ignored when online"]
    async fn live_price_pls_to_m3m3() {
        let client = NineMmClient::for_chain(369).unwrap();
        let req = AggQuoteRequest {
            token_in: Address::ZERO,
            token_out: address!("0x78a2809e8e2ef8e07429559f15703ee20e885588"),
            token_in_is_native: true,
            token_out_is_native: false,
            amount_in: U256::from(1_000_000_000_000_000_000u64),
            slippage_percent: 0.5,
            account: None,
        };
        let preview = client.preview_price(&req).await.unwrap();
        assert!(!preview.amount_out.is_zero());
    }

    #[tokio::test]
    #[ignore = "hits api.9mm.pro — run with --ignored when online"]
    async fn live_price_pls_to_hex() {
        let client = NineMmClient::for_chain(369).unwrap();
        let req = AggQuoteRequest {
            token_in: Address::ZERO,
            token_out: HEX,
            token_in_is_native: true,
            token_out_is_native: false,
            amount_in: U256::from(1_000_000_000_000_000_000u64),
            slippage_percent: 0.5,
            account: None,
        };
        let preview = client.preview_price(&req).await.unwrap();
        assert!(!preview.amount_out.is_zero());
        eprintln!(
            "9mm price: 1 PLS → ≈{} HEX ({}) sources={:?}",
            format_base_units(&preview.amount_out.to_string(), 8),
            preview.amount_out,
            preview.sources
        );
    }

    #[tokio::test]
    #[ignore = "hits api.9mm.pro — needs funded takerAddress on PulseChain"]
    async fn live_quote_pls_to_hex() {
        let client = NineMmClient::for_chain(369).unwrap();
        let taker = address!("0x14a54e673e626e25d8d8719005aec8c0992385e2");
        let req = AggQuoteRequest {
            token_in: Address::ZERO,
            token_out: HEX,
            token_in_is_native: true,
            token_out_is_native: false,
            amount_in: U256::from(1_000_000_000_000_000_000u64),
            slippage_percent: 0.5,
            account: Some(taker),
        };
        let q = client.quote(&req).await.unwrap();
        assert!(!q.tx.data.is_empty());
        eprintln!(
            "9mm quote: router={:#x} out={} value={}",
            q.tx.to, q.amount_out, q.tx.value
        );
    }

    #[tokio::test]
    #[ignore = "hits api.9mm.pro — run with --ignored when online"]
    async fn live_price_pls_to_usdc() {
        let client = NineMmClient::for_chain(369).unwrap();
        let req = AggQuoteRequest {
            token_in: Address::ZERO,
            token_out: USDC,
            token_in_is_native: true,
            token_out_is_native: false,
            amount_in: U256::from(1_000_000_000_000_000_000u64) * U256::from(1000u64),
            slippage_percent: 0.5,
            account: None,
        };
        let preview = client.preview_price(&req).await.unwrap();
        assert!(!preview.amount_out.is_zero());
        eprintln!(
            "9mm price: 1000 PLS → ≈{} USDC ({})",
            format_base_units(&preview.amount_out.to_string(), 6),
            preview.amount_out
        );
    }
}
