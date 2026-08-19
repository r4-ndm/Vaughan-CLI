//! 4-byte directory function signature reverse lookup.
//!
//! Queries the public 4byte.directory database to resolve raw 4-byte selectors
//! into human-readable text signatures (e.g. `0xa9059cbb` -> `transfer(address,uint256)`).

use super::selectors::{selector_to_hex, Selector};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct FourByteResult {
    id: u64,
    text_signature: String,
    hex_signature: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct FourByteResponse {
    count: u64,
    results: Vec<FourByteResult>,
}

/// Signature database client with in-memory caching.
#[derive(Debug)]
pub struct SignatureDb {
    http_client: reqwest::Client,
    cache: RwLock<HashMap<Selector, Vec<String>>>,
}

impl Default for SignatureDb {
    fn default() -> Self {
        Self::new()
    }
}

impl SignatureDb {
    /// Create a new SignatureDb client.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();

        let mut known = HashMap::new();
        // Seed common standard signatures so lookups are instantaneous offline
        known.insert(
            [0xa9, 0x05, 0x9c, 0xbb],
            vec!["transfer(address,uint256)".to_string()],
        );
        known.insert(
            [0x23, 0xb8, 0x72, 0xdd],
            vec!["transferFrom(address,address,uint256)".to_string()],
        );
        known.insert(
            [0x70, 0xa0, 0x82, 0x31],
            vec!["balanceOf(address)".to_string()],
        );
        known.insert(
            [0x09, 0x5e, 0xa7, 0xb3],
            vec!["approve(address,uint256)".to_string()],
        );
        known.insert(
            [0xdd, 0x62, 0xed, 0x3e],
            vec!["allowance(address,address)".to_string()],
        );
        known.insert([0x06, 0xfd, 0xde, 0x03], vec!["name()".to_string()]);
        known.insert([0x95, 0xd8, 0x9b, 0x41], vec!["symbol()".to_string()]);
        known.insert([0x31, 0x3c, 0xe7, 0xf2], vec!["decimals()".to_string()]);
        known.insert([0x18, 0x16, 0x0d, 0xdd], vec!["totalSupply()".to_string()]);
        known.insert([0x09, 0x02, 0xf1, 0xac], vec!["getReserves()".to_string()]);
        known.insert([0x38, 0x50, 0xc7, 0xbd], vec!["slot0()".to_string()]);
        known.insert([0x0d, 0xfe, 0x16, 0x81], vec!["token0()".to_string()]);
        known.insert([0xd2, 0x12, 0x20, 0xa7], vec!["token1()".to_string()]);
        known.insert([0xc4, 0x5a, 0x01, 0x53], vec!["factory()".to_string()]);
        known.insert(
            [0x57, 0x4e, 0xe4, 0xd9],
            vec!["allPairsLength()".to_string()],
        );
        known.insert(
            [0x1e, 0x3d, 0xd1, 0x8b],
            vec!["allPairs(uint256)".to_string()],
        );

        Self {
            http_client: client,
            cache: RwLock::new(known),
        }
    }

    /// Lookup text signatures for a 4-byte selector.
    ///
    /// Checks the cache first; queries 4byte.directory if not present.
    /// Returns matching text signatures or an empty vec if unresolved.
    pub async fn lookup(&self, selector: Selector) -> Vec<String> {
        // 1. Check in-memory cache
        if let Ok(guard) = self.cache.read() {
            if let Some(sigs) = guard.get(&selector) {
                return sigs.clone();
            }
        }

        // 2. Query 4byte.directory API
        let hex_str = selector_to_hex(selector);
        let url = format!(
            "https://www.4byte.directory/api/v1/signatures/?hex_signature={}",
            hex_str
        );

        match self.http_client.get(&url).send().await {
            Ok(resp) => {
                if let Ok(data) = resp.json::<FourByteResponse>().await {
                    let sigs: Vec<String> =
                        data.results.into_iter().map(|r| r.text_signature).collect();

                    if let Ok(mut guard) = self.cache.write() {
                        guard.insert(selector, sigs.clone());
                    }
                    return sigs;
                }
            }
            Err(err) => {
                tracing::debug!(selector = %hex_str, error = %err, "4byte directory lookup failed");
            }
        }

        Vec::new()
    }

    /// Best-effort name resolution: returns the first matching signature,
    /// or formats as `0x<hex>()` if unknown.
    pub async fn format_selector(&self, selector: Selector) -> String {
        let sigs = self.lookup(selector).await;
        if let Some(first) = sigs.first() {
            first.clone()
        } else {
            format!("{}()", selector_to_hex(selector))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn offline_lookup_common_signatures() {
        let db = SignatureDb::new();
        let transfer_sel = [0xa9, 0x05, 0x9c, 0xbb];
        let sigs = db.lookup(transfer_sel).await;
        assert_eq!(sigs, vec!["transfer(address,uint256)"]);

        let res = db.format_selector(transfer_sel).await;
        assert_eq!(res, "transfer(address,uint256)");
    }

    #[tokio::test]
    async fn fallback_for_unknown() {
        let db = SignatureDb::new();
        let unknown_sel = [0x11, 0x22, 0x33, 0x44];
        let res = db.format_selector(unknown_sel).await;
        assert_eq!(res, "0x11223344()");
    }
}
