//! Smart contract ABI resolution and disk caching.
//!
//! Fetches verified ABIs from block explorer APIs (e.g. PulseChain Scan) and
//! caches them locally on disk to eliminate redundant network roundtrips.

use alloy::json_abi::JsonAbi;
use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Result of attempting to resolve an ABI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AbiResolution {
    /// Contract is verified and has a valid ABI.
    Verified(JsonAbi),
    /// Contract is unverified on the explorer.
    Unverified,
    /// Explorer API or network error.
    Error(String),
}

/// Explorer API response wrapper.
#[derive(Debug, Deserialize)]
struct ExplorerResponse {
    status: String,
    message: String,
    result: String,
}

/// ABI Resolver with local disk cache.
#[derive(Debug, Clone)]
pub struct AbiResolver {
    cache_dir: PathBuf,
    http_client: reqwest::Client,
}

impl Default for AbiResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl AbiResolver {
    /// Create a new resolver with default cache directory (`~/.vaughan/cache/abis/`).
    pub fn new() -> Self {
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".vaughan")
            .join("cache")
            .join("abis");

        Self::with_cache_dir(base_dir)
    }

    /// Create a new resolver with a custom cache directory (useful for testing).
    pub fn with_cache_dir<P: AsRef<Path>>(cache_dir: P) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        Self {
            cache_dir: cache_dir.as_ref().to_path_buf(),
            http_client: client,
        }
    }

    /// Get the cache path for a specific contract on a given chain.
    fn cache_path(&self, chain_id: u64, address: Address) -> PathBuf {
        self.cache_dir
            .join(chain_id.to_string())
            .join(format!("{}.json", address.to_checksum(None)))
    }

    /// Resolve an ABI: checks disk cache first, then queries the block explorer.
    pub async fn resolve(&self, chain_id: u64, address: Address) -> AbiResolution {
        let cache_file = self.cache_path(chain_id, address);

        // 1. Check disk cache
        if cache_file.exists() {
            if let Ok(content) = fs::read_to_string(&cache_file) {
                if let Ok(abi) = serde_json::from_str::<JsonAbi>(&content) {
                    tracing::debug!(chain_id, %address, "ABI loaded from disk cache");
                    return AbiResolution::Verified(abi);
                }
            }
        }

        // 2. Fetch from Explorer API
        let endpoint = match get_explorer_api_url(chain_id, address) {
            Some(url) => url,
            None => {
                return AbiResolution::Error(format!(
                    "No block explorer ABI API configured for chain ID {}",
                    chain_id
                ))
            }
        };

        match self.http_client.get(&endpoint).send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    return AbiResolution::Error(format!(
                        "Explorer HTTP error: status {}",
                        resp.status()
                    ));
                }

                match resp.json::<ExplorerResponse>().await {
                    Ok(data) => {
                        if data.status == "1" {
                            match serde_json::from_str::<JsonAbi>(&data.result) {
                                Ok(abi) => {
                                    // Save to cache
                                    if let Some(parent) = cache_file.parent() {
                                        let _ = fs::create_dir_all(parent);
                                    }
                                    let _ = fs::write(&cache_file, &data.result);
                                    tracing::info!(chain_id, %address, "ABI fetched and cached");
                                    AbiResolution::Verified(abi)
                                }
                                Err(err) => AbiResolution::Error(format!(
                                    "Failed to parse ABI JSON: {}",
                                    err
                                )),
                            }
                        } else if data.result.to_lowercase().contains("not verified")
                            || data.message.to_lowercase().contains("not verified")
                        {
                            AbiResolution::Unverified
                        } else {
                            AbiResolution::Error(data.result)
                        }
                    }
                    Err(err) => {
                        AbiResolution::Error(format!("Failed to parse explorer response: {}", err))
                    }
                }
            }
            Err(err) => AbiResolution::Error(format!("Explorer network request failed: {}", err)),
        }
    }

    /// Save an ABI manually into the cache (e.g. for pre-loaded or standard ABIs).
    pub fn cache_abi(&self, chain_id: u64, address: Address, raw_json: &str) -> Result<(), String> {
        let cache_file = self.cache_path(chain_id, address);
        if let Some(parent) = cache_file.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(cache_file, raw_json).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Determine explorer API endpoint for a given chain ID.
fn get_explorer_api_url(chain_id: u64, address: Address) -> Option<String> {
    let addr_str = address.to_checksum(None);
    match chain_id {
        // PulseChain Mainnet
        369 => Some(format!(
            "https://api.scan.pulsechain.com/api?module=contract&action=getabi&address={}",
            addr_str
        )),
        // PulseChain Testnet v4
        943 => Some(format!(
            "https://api.v4.testnet.pulsechain.com/api?module=contract&action=getabi&address={}",
            addr_str
        )),
        // Ethereum Mainnet
        1 => Some(format!(
            "https://api.etherscan.io/api?module=contract&action=getabi&address={}",
            addr_str
        )),
        // Sepolia Testnet
        11155111 => Some(format!(
            "https://api-sepolia.etherscan.io/api?module=contract&action=getabi&address={}",
            addr_str
        )),
        // Polygon
        137 => Some(format!(
            "https://api.polygonscan.com/api?module=contract&action=getabi&address={}",
            addr_str
        )),
        // BSC
        56 => Some(format!(
            "https://api.bscscan.com/api?module=contract&action=getabi&address={}",
            addr_str
        )),
        // Base
        8453 => Some(format!(
            "https://api.basescan.org/api?module=contract&action=getabi&address={}",
            addr_str
        )),
        // Arbitrum One
        42161 => Some(format!(
            "https://api.arbiscan.io/api?module=contract&action=getabi&address={}",
            addr_str
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const ERC20_SAMPLE_ABI: &str = r#"[
        {
            "constant": true,
            "inputs": [],
            "name": "name",
            "outputs": [{"name": "", "type": "string"}],
            "payable": false,
            "stateMutability": "view",
            "type": "function"
        },
        {
            "constant": true,
            "inputs": [],
            "name": "totalSupply",
            "outputs": [{"name": "", "type": "uint256"}],
            "payable": false,
            "stateMutability": "view",
            "type": "function"
        }
    ]"#;

    #[tokio::test]
    async fn cache_roundtrip() {
        let tmp = tempdir().unwrap();
        let resolver = AbiResolver::with_cache_dir(tmp.path());
        let addr = Address::repeat_byte(0x42);

        // Manually cache sample ABI
        resolver.cache_abi(369, addr, ERC20_SAMPLE_ABI).unwrap();

        // Resolve should return Verified from cache without hitting network
        match resolver.resolve(369, addr).await {
            AbiResolution::Verified(abi) => {
                assert_eq!(abi.functions.len(), 2);
                assert!(abi.functions.contains_key("name"));
                assert!(abi.functions.contains_key("totalSupply"));
            }
            other => panic!("Expected Verified, got {:?}", other),
        }
    }

    #[test]
    fn explorer_url_mapping() {
        let addr = Address::repeat_byte(0x11);
        let url = get_explorer_api_url(369, addr).unwrap();
        assert!(url.contains("api.scan.pulsechain.com"));
        assert!(url.contains(&addr.to_checksum(None)));
    }
}
