//! HTTP client for LibertySwap `v3/swap/quote`.

use alloy::primitives::Address;
use std::collections::HashSet;
use std::sync::OnceLock;

use crate::error::WalletError;

use super::types::{BridgeQuote, BridgeQuoteRequest, WireQuote};

/// Live web-app API root (not the outdated docs host `api.libertyswap.finance`).
pub const LIBERTY_SWAP_V3_BASE: &str = "https://apis.libertyswap.finance/v3";

/// Official routers from Liberty docs + current unified router seen on live quotes.
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
];

fn router_set() -> &'static HashSet<[u8; 20]> {
    static SET: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();
    SET.get_or_init(|| {
        OFFICIAL_ROUTERS
            .iter()
            .filter_map(|s| s.parse::<Address>().ok())
            .map(|a| a.into_array())
            .collect()
    })
}

/// True when `to` is on the known Liberty router allowlist.
pub fn is_whitelisted_router(to: Address) -> bool {
    router_set().contains(&to.into_array())
}

/// Thin reqwest wrapper around LibertySwap v3 quote.
pub struct LibertySwapClient {
    http: reqwest::Client,
    base: String,
}

impl LibertySwapClient {
    /// Public API (no partner key).
    pub fn public() -> Result<Self, WalletError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("vaughan-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| WalletError::NetworkError(format!("liberty http: {e}")))?;
        Ok(Self {
            http,
            base: LIBERTY_SWAP_V3_BASE.trim_end_matches('/').to_string(),
        })
    }

    /// Fetch a bridge quote + source-chain calldata. Does not sign or broadcast.
    pub async fn quote(&self, req: &BridgeQuoteRequest) -> Result<BridgeQuote, WalletError> {
        if req.src_chain == req.dst_chain {
            return Err(WalletError::InvalidTransaction(
                "liberty: src and dst chain must differ".into(),
            ));
        }
        let src = req.src_token.as_query();
        let dst = req.dst_token.as_query();
        let url = format!(
            "{}/swap/quote?srcToken={src}&dstToken={dst}&amount={}&srcChain={}&dstChain={}&recipient={:#x}",
            self.base, req.amount, req.src_chain, req.dst_chain, req.recipient
        );

        let resp = self
            .http
            .get(url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| WalletError::NetworkError(format!("liberty quote: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| WalletError::NetworkError(format!("liberty body: {e}")))?;

        if status.as_u16() == 429 {
            return Err(WalletError::NetworkError(
                "liberty rate limited (429) — ~30 req/min; slow down".into(),
            ));
        }
        if !status.is_success() {
            let snippet: String = text.chars().take(180).collect();
            return Err(WalletError::NetworkError(format!(
                "liberty HTTP {status}: {snippet}"
            )));
        }

        let wire: WireQuote = serde_json::from_str(&text)
            .map_err(|e| WalletError::Serialization(format!("liberty quote JSON: {e}")))?;
        let quote = wire.into_bridge_quote()?;
        assert_bridge_exec_targets(&quote)?;
        if quote.dest_amount.is_zero() {
            return Err(WalletError::NetworkError("liberty: zero destAmount".into()));
        }
        Ok(quote)
    }
}

/// Refuse quotes whose router / approve spender are not allowlisted (or diverge).
pub fn assert_bridge_exec_targets(quote: &BridgeQuote) -> Result<(), WalletError> {
    if !is_whitelisted_router(quote.to) {
        return Err(WalletError::InvalidTransaction(format!(
            "liberty: router {:#x} not on allowlist — refusing to quote",
            quote.to
        )));
    }
    if let Some(ref ap) = quote.approval {
        if !is_whitelisted_router(ap.spender) {
            return Err(WalletError::InvalidTransaction(format!(
                "liberty: approval spender {:#x} not on allowlist — refusing to quote",
                ap.spender
            )));
        }
        if ap.spender != quote.to {
            return Err(WalletError::InvalidTransaction(format!(
                "liberty: approval spender {:#x} != router {:#x} — refusing to quote",
                ap.spender, quote.to
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::bridge::types::{BridgeAsset, BridgeQuoteRequest, WireQuote};
    use crate::core::format_base_units;
    use alloy::primitives::{address, U256};

    #[test]
    fn parses_v3_fixture_and_whitelists_router() {
        let raw = r#"{
            "to": "0x78f63fe16C83728c16C0aE44b0c19D7dD105c215",
            "srcToken": {
                "symbol": "USDC",
                "address": "0x15D38573d2feeb82e7ad5187aB8c1D52810B1f07",
                "decimals": 6,
                "chainId": 369
            },
            "destToken": {
                "symbol": "USDC",
                "address": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "decimals": 6,
                "chainId": 8453
            },
            "srcAmount": "1000000000",
            "destAmount": "995000000",
            "fee": {
                "chainId": 8453,
                "token": "destination",
                "protocol": { "percentage": 0.5, "amount": "5000000" },
                "integrator": { "percentage": 0, "amount": "0" },
                "total": { "amount": "5000000" }
            },
            "approval": {
                "token": "0x15D38573d2feeb82e7ad5187aB8c1D52810B1f07",
                "spender": "0x78f63fe16C83728c16C0aE44b0c19D7dD105c215",
                "amount": "1000000000",
                "calldata": "0x095ea7b3"
            },
            "methodParameters": {
                "calldata": "0x095ff22d00000001",
                "value": "0x00"
            },
            "route": { "type": "DIRECT" }
        }"#;
        let wire: WireQuote = serde_json::from_str(raw).unwrap();
        let q = wire.into_bridge_quote().unwrap();
        assert!(is_whitelisted_router(q.to));
        assert_eq!(q.dest_amount, U256::from(995_000_000u64));
        assert!(q.approval.is_some());
        assert_eq!(q.fee.percentage, 0.5);
    }

    #[test]
    fn rejects_unknown_router() {
        assert!(!is_whitelisted_router(address!(
            "0x1111111111111111111111111111111111111111"
        )));
    }

    #[test]
    fn rejects_approval_spender_mismatch() {
        let raw = r#"{
            "to": "0x78f63fe16C83728c16C0aE44b0c19D7dD105c215",
            "srcToken": {
                "symbol": "USDC",
                "address": "0x15D38573d2feeb82e7ad5187aB8c1D52810B1f07",
                "decimals": 6,
                "chainId": 369
            },
            "destToken": {
                "symbol": "USDC",
                "address": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "decimals": 6,
                "chainId": 8453
            },
            "srcAmount": "1000000000",
            "destAmount": "995000000",
            "fee": {
                "chainId": 8453,
                "token": "destination",
                "protocol": { "percentage": 0.5, "amount": "5000000" },
                "integrator": { "percentage": 0, "amount": "0" },
                "total": { "amount": "5000000" }
            },
            "approval": {
                "token": "0x15D38573d2feeb82e7ad5187aB8c1D52810B1f07",
                "spender": "0x1111111111111111111111111111111111111111",
                "amount": "1000000000",
                "calldata": "0x095ea7b3"
            },
            "methodParameters": {
                "calldata": "0x095ff22d00000001",
                "value": "0x00"
            },
            "route": { "type": "DIRECT" }
        }"#;
        let wire: WireQuote = serde_json::from_str(raw).unwrap();
        let q = wire.into_bridge_quote().unwrap();
        let err = assert_bridge_exec_targets(&q).unwrap_err();
        assert!(
            err.to_string().contains("spender") || err.to_string().contains("allowlist"),
            "{err}"
        );
    }

    /// Live mainnet quote — no funds, no broadcast.
    #[tokio::test]
    #[ignore = "hits apis.libertyswap.finance — run with --ignored when online"]
    async fn live_quote_usdc_pulse_to_base() {
        let client = LibertySwapClient::public().unwrap();
        let recipient = address!("0x1111111111111111111111111111111111111111");
        let req = BridgeQuoteRequest {
            src_token: BridgeAsset::Symbol("USDC"),
            dst_token: BridgeAsset::Symbol("USDC"),
            amount: U256::from(1_000_000_000u64), // 1000 USDC (6 dp) — within Liberty limits
            src_chain: 369,
            dst_chain: 8453,
            recipient,
        };
        // Min is 10 USDC — use 100 USDC
        let req = BridgeQuoteRequest {
            amount: U256::from(100_000_000u64),
            ..req
        };
        let q = client.quote(&req).await.unwrap();
        assert!(is_whitelisted_router(q.to));
        eprintln!(
            "=== LibertySwap quote (NOT broadcast) ===\n\
             {} USDC on Pulse → ≈{} USDC on Base\n\
             fee {:.2}% · router {:#x}\n\
             calldata {} bytes · approve={}",
            format_base_units(&q.src_amount.to_string(), q.src_token.decimals),
            format_base_units(&q.dest_amount.to_string(), q.dest_token.decimals),
            q.fee.percentage,
            q.to,
            q.tx.data.len(),
            q.approval.is_some()
        );
    }
}
