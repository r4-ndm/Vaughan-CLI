//! HTTP client for the public DexScreener API (no API key).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::WalletError;

use super::chain::{catalog_chain_id_for_dex_slug, resolve_dex_chain, DEFAULT_DEXSCREENER_CHAIN};
use super::search::{
    attach_origin_labels, build_catalog_search_coverage, compose_search_guidance,
    rank_and_annotate_search_pairs,
};
use super::types::{DexPairSummary, DexScreenerSoftFail, DexTokenSide, SearchSuccess};

/// Official DexScreener API base.
pub const DEXSCREENER_API_BASE: &str = "https://api.dexscreener.com";

const MIN_INTERVAL: Duration = Duration::from_millis(200);

/// Thin reqwest wrapper with process-local outbound spacing.
pub struct DexScreenerClient {
    http: reqwest::Client,
    last_request: Arc<Mutex<Option<Instant>>>,
}

impl DexScreenerClient {
    pub fn new() -> Result<Self, WalletError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("vaughan-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| WalletError::NetworkError(format!("dexscreener http client: {e}")))?;
        Ok(Self {
            http,
            last_request: Arc::new(Mutex::new(None)),
        })
    }

    /// Discovery-only search. Defaults to PulseChain-only filtering.
    pub async fn search(
        &self,
        query: &str,
        chain_id: Option<u64>,
        dex_chain: Option<&str>,
        pulsechain_only: bool,
    ) -> Result<Value, WalletError> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(serde_json::to_value(
                DexScreenerSoftFail::new("Search query is empty").with_path("/latest/dex/search"),
            )
            .unwrap_or_default());
        }
        let slug = resolve_dex_chain(chain_id, dex_chain);
        let url = format!(
            "{DEXSCREENER_API_BASE}/latest/dex/search?q={}",
            urlencoding_encode(q)
        );
        let body = match self.get_json(&url).await {
            Ok(v) => v,
            Err(soft) => return Ok(serde_json::to_value(soft).unwrap_or_default()),
        };

        let mut pairs = normalize_pair_list(&body);
        if pulsechain_only {
            pairs.retain(|p| p.chain_id.eq_ignore_ascii_case(&slug));
        }
        let catalog_cid = catalog_chain_id_for_dex_slug(&slug).unwrap_or(369);
        let (ranked, collisions) = rank_and_annotate_search_pairs(pairs, catalog_cid);
        let (coverage, followups) = build_catalog_search_coverage(q, &ranked);
        let guidance = compose_search_guidance(coverage.as_ref(), &followups);

        let success = SearchSuccess {
            ok: true,
            source: "dexscreener",
            chain_id: slug,
            pulsechain_only,
            pair_count: ranked.len(),
            query: q.to_string(),
            pairs: ranked,
            discovery_only: true,
            guidance,
            symbol_collisions: collisions,
            catalog_coverage: coverage,
            recommended_address_followups: if followups.is_empty() {
                None
            } else {
                Some(followups)
            },
        };
        Ok(serde_json::to_value(success).unwrap_or_default())
    }

    /// Pairs for a token address on a chain.
    pub async fn token_pairs(
        &self,
        token: &str,
        chain_id: Option<u64>,
        dex_chain: Option<&str>,
    ) -> Result<Value, WalletError> {
        let slug = resolve_dex_chain(chain_id, dex_chain);
        let token = token.trim();
        if !is_hex_address(token) {
            return Ok(serde_json::to_value(
                DexScreenerSoftFail::new("tokenAddress must be a 0x…40-hex address")
                    .with_path("/token-pairs/v1")
                    .with_chain(&slug),
            )
            .unwrap_or_default());
        }
        let url = format!("{DEXSCREENER_API_BASE}/token-pairs/v1/{slug}/{token}");
        let body = match self.get_json(&url).await {
            Ok(v) => v,
            Err(soft) => return Ok(serde_json::to_value(soft).unwrap_or_default()),
        };
        let mut pairs = normalize_pair_list(&body);
        let catalog_cid = catalog_chain_id_for_dex_slug(&slug);
        for p in &mut pairs {
            attach_origin_labels(p, catalog_cid);
        }
        Ok(serde_json::json!({
            "ok": true,
            "source": "dexscreener",
            "chain_id": slug,
            "token": token,
            "pair_count": pairs.len(),
            "pairs": pairs,
            "guidance": "Address-keyed identity. Origin labels only for catalogued tokens.",
        }))
    }

    /// Single pair by LP address.
    pub async fn pair(
        &self,
        pair_address: &str,
        chain_id: Option<u64>,
        dex_chain: Option<&str>,
    ) -> Result<Value, WalletError> {
        let slug = resolve_dex_chain(chain_id, dex_chain);
        let pair_address = pair_address.trim();
        if !is_hex_address(pair_address) {
            return Ok(serde_json::to_value(
                DexScreenerSoftFail::new("pairAddress must be a 0x…40-hex address")
                    .with_path("/latest/dex/pairs")
                    .with_chain(&slug),
            )
            .unwrap_or_default());
        }
        let url = format!("{DEXSCREENER_API_BASE}/latest/dex/pairs/{slug}/{pair_address}");
        let body = match self.get_json(&url).await {
            Ok(v) => v,
            Err(soft) => return Ok(serde_json::to_value(soft).unwrap_or_default()),
        };
        let mut pairs = normalize_pair_list(&body);
        let catalog_cid = catalog_chain_id_for_dex_slug(&slug);
        for p in &mut pairs {
            attach_origin_labels(p, catalog_cid);
        }
        let pair = pairs.into_iter().next();
        Ok(serde_json::json!({
            "ok": true,
            "source": "dexscreener",
            "chain_id": slug,
            "pair": pair,
        }))
    }

    /// Batch token → pairs (max 30).
    pub async fn tokens(
        &self,
        tokens: &[String],
        chain_id: Option<u64>,
        dex_chain: Option<&str>,
    ) -> Result<Value, WalletError> {
        let slug = resolve_dex_chain(chain_id, dex_chain);
        let mut cleaned: Vec<String> = tokens
            .iter()
            .map(|t| t.trim().to_string())
            .filter(|t| is_hex_address(t))
            .take(30)
            .collect();
        cleaned.sort();
        cleaned.dedup();
        if cleaned.is_empty() {
            return Ok(serde_json::to_value(
                DexScreenerSoftFail::new("Provide 1–30 token addresses")
                    .with_path("/tokens/v1")
                    .with_chain(&slug),
            )
            .unwrap_or_default());
        }
        let joined = cleaned.join(",");
        let url = format!("{DEXSCREENER_API_BASE}/tokens/v1/{slug}/{joined}");
        let body = match self.get_json(&url).await {
            Ok(v) => v,
            Err(soft) => return Ok(serde_json::to_value(soft).unwrap_or_default()),
        };
        let mut pairs = normalize_pair_list(&body);
        let catalog_cid = catalog_chain_id_for_dex_slug(&slug);
        for p in &mut pairs {
            attach_origin_labels(p, catalog_cid);
        }
        Ok(serde_json::json!({
            "ok": true,
            "source": "dexscreener",
            "chain_id": slug,
            "tokens": cleaned,
            "pair_count": pairs.len(),
            "pairs": pairs,
        }))
    }

    async fn get_json(&self, url: &str) -> Result<Value, DexScreenerSoftFail> {
        {
            let mut guard = self.last_request.lock().await;
            if let Some(prev) = *guard {
                let elapsed = prev.elapsed();
                if elapsed < MIN_INTERVAL {
                    tokio::time::sleep(MIN_INTERVAL - elapsed).await;
                }
            }
            *guard = Some(Instant::now());
        }

        let resp = self.http.get(url).send().await.map_err(|e| {
            DexScreenerSoftFail::new(format!("network: {e}")).with_path(url_path(url))
        })?;
        let status = resp.status().as_u16();
        if status == 429 {
            return Err(DexScreenerSoftFail::new("rate limited (429)")
                .with_status(429)
                .with_path(url_path(url)));
        }
        if !(200..300).contains(&status) {
            return Err(DexScreenerSoftFail::new(format!("HTTP {status}"))
                .with_status(status)
                .with_path(url_path(url)));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| DexScreenerSoftFail::new(format!("json: {e}")).with_path(url_path(url)))
    }
}

impl Default for DexScreenerClient {
    fn default() -> Self {
        Self::new().expect("dexscreener client")
    }
}

fn url_path(url: &str) -> String {
    url.split('?').next().unwrap_or(url).to_string()
}

fn urlencoding_encode(s: &str) -> String {
    // Minimal encode for search queries (spaces + reserved).
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn is_hex_address(s: &str) -> bool {
    let s = s.trim();
    s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

fn normalize_pair_list(body: &Value) -> Vec<DexPairSummary> {
    let arr = if let Some(a) = body.get("pairs").and_then(|v| v.as_array()) {
        a.clone()
    } else if let Some(a) = body.as_array() {
        a.clone()
    } else if let Some(p) = body.get("pair") {
        vec![p.clone()]
    } else {
        Vec::new()
    };
    arr.iter().filter_map(normalize_pair).collect()
}

fn normalize_pair(v: &Value) -> Option<DexPairSummary> {
    let chain_id = v.get("chainId")?.as_str()?.to_string();
    let dex_id = v
        .get("dexId")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let url = v
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let pair_address = v.get("pairAddress")?.as_str()?.to_string();
    let base = token_side(v.get("baseToken")?)?;
    let quote = token_side(v.get("quoteToken")?)?;
    let price_usd = v.get("priceUsd").and_then(|x| {
        x.as_str()
            .map(|s| s.to_string())
            .or_else(|| x.as_f64().map(|f| f.to_string()))
    });
    let liquidity_usd = v
        .get("liquidity")
        .and_then(|l| l.get("usd"))
        .and_then(|x| x.as_f64());
    let volume_h24 = v
        .get("volume")
        .and_then(|vol| vol.get("h24"))
        .and_then(|x| x.as_f64());

    let mut pair = DexPairSummary {
        chain_id,
        dex_id,
        url,
        pair_address,
        base_token: base,
        quote_token: quote,
        price_usd,
        liquidity_usd,
        volume_h24,
        search_flags: None,
    };
    if let Some(cid) = catalog_chain_id_for_dex_slug(&pair.chain_id) {
        attach_origin_labels(&mut pair, Some(cid));
    }
    Some(pair)
}

fn token_side(v: &Value) -> Option<DexTokenSide> {
    Some(DexTokenSide {
        address: v.get("address")?.as_str()?.to_string(),
        name: v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        symbol: v
            .get("symbol")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        origin: None,
    })
}

/// Default Dex chain slug (PulseChain).
pub fn default_chain_slug() -> &'static str {
    DEFAULT_DEXSCREENER_CHAIN
}
