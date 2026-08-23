//! HTTP quote client for Piteas Pathfinder.

use secrecy::{ExposeSecret, SecretString};
use url::Url;

use crate::error::WalletError;

use super::config::{AuthStyle, PiteasFileConfig};
use super::types::{PiteasQuote, QuoteRequest};

/// Default path on the SDK host.
pub const DEFAULT_QUOTE_PATH: &str = "/quote";

/// Thin reqwest wrapper around the Piteas quote endpoint.
pub struct PiteasClient {
    http: reqwest::Client,
    base_url: String,
    auth_style: AuthStyle,
    api_key: Option<SecretString>,
}

impl PiteasClient {
    /// Build from on-disk config + optional decrypted partner key.
    pub fn from_config(
        cfg: &PiteasFileConfig,
        api_key: Option<SecretString>,
    ) -> Result<Self, WalletError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("vaughan-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| WalletError::NetworkError(format!("piteas http client: {e}")))?;
        Ok(Self {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            auth_style: cfg.auth_style,
            api_key,
        })
    }

    /// Public SDK defaults (no partner key).
    pub fn public_beta() -> Result<Self, WalletError> {
        Self::from_config(&PiteasFileConfig::default(), None)
    }

    /// Fetch a swap quote (calldata + amounts). Does not sign or broadcast.
    pub async fn quote(&self, req: &QuoteRequest) -> Result<PiteasQuote, WalletError> {
        let url = self.build_quote_url(req)?;
        let mut builder = self.http.get(url);
        builder = self.apply_auth(builder)?;

        let resp = builder
            .send()
            .await
            .map_err(|e| WalletError::NetworkError(format!("piteas quote request: {e}")))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| WalletError::NetworkError(format!("piteas quote body: {e}")))?;

        if status.as_u16() == 429 {
            return Err(WalletError::NetworkError(
                "piteas rate limit (429) — beta allows ~10 req/min; wait before retrying".into(),
            ));
        }
        if status.as_u16() == 403 {
            return Err(WalletError::NetworkError(
                "piteas forbidden (403) — IP or partner access blocked; contact Piteas".into(),
            ));
        }
        if !status.is_success() {
            let snippet: String = body.chars().take(200).collect();
            return Err(WalletError::NetworkError(format!(
                "piteas quote HTTP {status}: {snippet}"
            )));
        }

        serde_json::from_str(&body)
            .map_err(|e| WalletError::Serialization(format!("piteas quote JSON: {e}")))
    }

    fn build_quote_url(&self, req: &QuoteRequest) -> Result<Url, WalletError> {
        let mut url = Url::parse(&format!("{}{DEFAULT_QUOTE_PATH}", self.base_url))
            .map_err(|e| WalletError::Other(format!("piteas base_url: {e}")))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("tokenInAddress", &req.token_in.to_string());
            q.append_pair("tokenOutAddress", &req.token_out.to_string());
            q.append_pair("amount", &req.amount.to_string());
            q.append_pair("allowedSlippage", &format!("{:.2}", req.allowed_slippage));
            if let Some(account) = req.account {
                q.append_pair("account", &account.to_string());
            }
            if self.auth_style == AuthStyle::Query {
                if let Some(ref key) = self.api_key {
                    q.append_pair("apiKey", key.expose_secret());
                }
            }
        }
        Ok(url)
    }

    fn apply_auth(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, WalletError> {
        let Some(ref key) = self.api_key else {
            return Ok(builder);
        };
        let secret = key.expose_secret();
        Ok(match self.auth_style {
            AuthStyle::None | AuthStyle::Query => builder,
            AuthStyle::Bearer => builder.header("Authorization", format!("Bearer {secret}")),
            AuthStyle::XApiKey => builder.header("X-API-Key", secret),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::piteas::types::NativeToken;
    use alloy::primitives::{address, U256};

    #[test]
    fn quote_url_encodes_pls_and_amount() {
        let client = PiteasClient::public_beta().unwrap();
        let req = QuoteRequest::new(
            NativeToken::Pls,
            NativeToken::Address(address!("0xefD766cCb38EaF1dfd701853BFCe31359239F305")),
            U256::from(1_000_000_000_000_000_000u64),
        )
        .with_slippage(0.5);
        let url = client.build_quote_url(&req).unwrap();
        let s = url.as_str();
        assert!(s.contains("tokenInAddress=PLS"));
        assert!(s.contains("amount=1000000000000000000"));
        assert!(s.contains("allowedSlippage=0.50"));
    }

    #[test]
    fn query_auth_appends_api_key() {
        let cfg = PiteasFileConfig {
            auth_style: AuthStyle::Query,
            ..Default::default()
        };
        let client =
            PiteasClient::from_config(&cfg, Some(SecretString::from("secret-key".to_string())))
                .unwrap();
        let req = QuoteRequest::new(
            NativeToken::Pls,
            NativeToken::Address(address!("0xefD766cCb38EaF1dfd701853BFCe31359239F305")),
            U256::from(1u64),
        );
        let url = client.build_quote_url(&req).unwrap();
        assert!(url.as_str().contains("apiKey=secret-key"));
    }
}

/// Live mainnet quote — ignored by default (network + rate limits).
#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::core::piteas::types::NativeToken;
    use alloy::primitives::{address, U256};

    #[tokio::test]
    #[ignore = "hits sdk.piteas.io — run with --ignored when online"]
    async fn live_public_quote_pls_to_dai() {
        let client = PiteasClient::public_beta().unwrap();
        let quote = client
            .quote(
                &QuoteRequest::new(
                    NativeToken::Pls,
                    NativeToken::Address(address!("0xefD766cCb38EaF1dfd701853BFCe31359239F305")),
                    U256::from(1_000_000_000_000_000_000u64),
                )
                .with_slippage(0.5),
            )
            .await
            .unwrap();
        assert_eq!(quote.dest_token.symbol, "DAI");
        assert!(!quote.method_parameters.calldata.is_empty());
        assert!(quote.method_parameters.value_u256().unwrap() > U256::ZERO);
    }
}
