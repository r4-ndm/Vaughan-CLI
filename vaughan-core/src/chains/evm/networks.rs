//! Built-in EVM network configurations.
//!
//! PulseChain is the primary target; Ethereum and other common EVM chains are
//! included so the wallet is multi-chain ready out of the box.

use serde::{Deserialize, Serialize};

/// A selectable RPC endpoint (preset label + URL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcEndpoint {
    /// Human-readable label (e.g. "Official", "PublicNode").
    pub label: String,
    /// HTTPS (or dev http) JSON-RPC URL.
    pub url: String,
}

/// Resolve the primary RPC and ordered fallbacks for a network.
///
/// Precedence: `session_override` (CLI `--rpc-url`) → `persisted_primary` (Settings)
/// → built-in default. Fallbacks are every other known endpoint for the network.
pub fn resolve_rpc_endpoints(
    net: &EvmNetworkConfig,
    persisted_primary: Option<&str>,
    session_override: Option<&str>,
) -> (String, Vec<String>) {
    let primary = session_override
        .map(str::to_string)
        .or_else(|| persisted_primary.map(str::to_string))
        .unwrap_or_else(|| net.rpc_url.clone());

    let mut fallbacks = Vec::new();
    for endpoint in net.known_rpc_endpoints() {
        if endpoint.url != primary && !fallbacks.contains(&endpoint.url) {
            fallbacks.push(endpoint.url);
        }
    }
    (primary, fallbacks)
}

/// Short label for a known RPC URL (hostname fallback for custom URLs).
pub fn rpc_endpoint_label(url: &str) -> String {
    match url {
        "https://rpc.pulsechain.com" => "Official".into(),
        "https://pulsechain-rpc.publicnode.com" => "PublicNode".into(),
        "https://rpc.pulsechainrpc.com" => "PulseChainRPC".into(),
        "https://rpc.pulsechainstats.com" => "PulseChain Stats".into(),
        "https://rpc-pulsechain.g4mm4.io" => "g4mm4".into(),
        "https://rpc.gigatheminter.com" => "GigaTheMinter".into(),
        "https://rpc.v4.testnet.pulsechain.com" => "Official testnet".into(),
        "https://rpc-testnet-v4.g4mm4.io" => "g4mm4 testnet".into(),
        "https://pulsechain-testnet-rpc.publicnode.com" => "PublicNode testnet".into(),
        "https://eth.llamarpc.com" => "LlamaRPC".into(),
        "https://ethereum-rpc.publicnode.com" => "PublicNode".into(),
        "https://rpc.ankr.com/eth" => "Ankr".into(),
        "https://ethereum-sepolia-rpc.publicnode.com" => "PublicNode".into(),
        "https://rpc.sepolia.org" => "Sepolia.org".into(),
        "https://sepolia.drpc.org" => "dRPC".into(),
        "https://polygon-bor-rpc.publicnode.com" => "PublicNode".into(),
        "https://polygon-rpc.com" => "Polygon".into(),
        "https://bsc-dataseed.binance.org" => "Binance 1".into(),
        "https://bsc-dataseed1.binance.org" => "Binance 2".into(),
        "https://bsc-dataseed2.binance.org" => "Binance 3".into(),
        "https://bsc-dataseed3.binance.org" => "Binance 4".into(),
        "https://mainnet.base.org" => "Base official".into(),
        "https://base-rpc.publicnode.com" => "PublicNode".into(),
        other => url::Url::parse(other)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "Custom".into()),
    }
}

/// EVM network configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmNetworkConfig {
    /// Stable identifier (e.g. "ethereum", "pulsechain").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Chain ID.
    pub chain_id: u64,
    /// Default RPC URL.
    pub rpc_url: String,
    /// Fallback RPC URLs tried (in order) when the primary fails.
    #[serde(default)]
    pub fallback_rpc_urls: Vec<String>,
    /// Block explorer base URL.
    pub explorer_url: Option<String>,
    /// Native token symbol (e.g. ETH, PLS).
    pub native_symbol: String,
    /// Native token name.
    pub native_name: String,
    /// Native token decimals (18 for most EVM chains).
    pub decimals: u8,
    /// Default EIP-1559 priority fee (tip) in wei for fee estimation.
    /// `None` falls back to the adapter's generic default (1.5 gwei).
    #[serde(default)]
    pub default_priority_fee_wei: Option<u64>,
    /// True for test networks.
    pub is_testnet: bool,
}

impl EvmNetworkConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        chain_id: u64,
        rpc_url: impl Into<String>,
        native_symbol: impl Into<String>,
        native_name: impl Into<String>,
        is_testnet: bool,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            chain_id,
            rpc_url: rpc_url.into(),
            fallback_rpc_urls: Vec::new(),
            explorer_url: None,
            native_symbol: native_symbol.into(),
            native_name: native_name.into(),
            decimals: 18,
            default_priority_fee_wei: None,
            is_testnet,
        }
    }

    pub fn with_explorer(mut self, explorer_url: impl Into<String>) -> Self {
        self.explorer_url = Some(explorer_url.into());
        self
    }

    pub fn with_fallback_rpcs(mut self, urls: &[&str]) -> Self {
        self.fallback_rpc_urls = urls.iter().map(|u| u.to_string()).collect();
        self
    }

    pub fn with_priority_fee_wei(mut self, wei: u64) -> Self {
        self.default_priority_fee_wei = Some(wei);
        self
    }

    /// ERC-3770 chain short name used in `st:<short>:` stealth URIs.
    pub fn eip3770_short_name(&self) -> &'static str {
        eip3770_short_name(self.chain_id)
    }

    /// Built-in primary plus fallbacks as selectable presets (deduped).
    pub fn known_rpc_endpoints(&self) -> Vec<RpcEndpoint> {
        let mut out = Vec::new();
        for url in std::iter::once(self.rpc_url.as_str())
            .chain(self.fallback_rpc_urls.iter().map(String::as_str))
        {
            if out.iter().any(|e: &RpcEndpoint| e.url == url) {
                continue;
            }
            out.push(RpcEndpoint {
                label: rpc_endpoint_label(url),
                url: url.to_string(),
            });
        }
        out
    }
}

/// ERC-3770 short name for a chain id (`pls`, `tpls`, `eth`, …).
pub fn eip3770_short_name(chain_id: u64) -> &'static str {
    match chain_id {
        1 => "eth",
        56 => "bnb",
        137 => "matic",
        369 => "pls",
        8453 => "base",
        943 => "tpls",
        11155111 => "sep",
        _ => "eth",
    }
}

/// Ethereum mainnet.
pub fn ethereum_mainnet() -> EvmNetworkConfig {
    EvmNetworkConfig::new(
        "ethereum",
        "Ethereum Mainnet",
        1,
        "https://eth.llamarpc.com",
        "ETH",
        "Ethereum",
        false,
    )
    .with_fallback_rpcs(&[
        "https://ethereum-rpc.publicnode.com",
        "https://rpc.ankr.com/eth",
    ])
    .with_priority_fee_wei(1_500_000_000) // 1.5 gwei
    .with_explorer("https://etherscan.io")
}

/// PulseChain mainnet (chain id 369).
///
/// Gas is typically fractions of a gwei, so the default priority fee is 0.01
/// gwei instead of Ethereum's 1.5 gwei (audit finding 4.2).
pub fn pulsechain_mainnet() -> EvmNetworkConfig {
    EvmNetworkConfig::new(
        "pulsechain",
        "PulseChain Mainnet",
        369,
        "https://rpc.pulsechain.com",
        "PLS",
        "PulseChain",
        false,
    )
    .with_fallback_rpcs(&[
        "https://pulsechain-rpc.publicnode.com",
        "https://rpc.pulsechainrpc.com",
        "https://rpc.pulsechainstats.com",
        "https://rpc-pulsechain.g4mm4.io",
        "https://rpc.gigatheminter.com",
    ])
    .with_priority_fee_wei(10_000_000) // 0.01 gwei
    .with_explorer("https://scan.pulsechain.com")
}

/// PulseChain testnet v4 (chain id 943).
pub fn pulsechain_testnet_v4() -> EvmNetworkConfig {
    EvmNetworkConfig::new(
        "pulsechain-testnet-v4",
        "PulseChain Testnet V4",
        943,
        "https://rpc.v4.testnet.pulsechain.com",
        "tPLS",
        "Test PulseChain",
        true,
    )
    .with_fallback_rpcs(&[
        "https://rpc-testnet-v4.g4mm4.io",
        "https://pulsechain-testnet-rpc.publicnode.com",
    ])
    .with_priority_fee_wei(10_000_000) // 0.01 gwei
    .with_explorer("https://scan.v4.testnet.pulsechain.com")
}

/// Ethereum Sepolia testnet.
pub fn ethereum_sepolia() -> EvmNetworkConfig {
    EvmNetworkConfig::new(
        "sepolia",
        "Ethereum Sepolia",
        11_155_111,
        "https://ethereum-sepolia-rpc.publicnode.com",
        "ETH",
        "Sepolia Ether",
        true,
    )
    .with_fallback_rpcs(&["https://rpc.sepolia.org", "https://sepolia.drpc.org"])
    .with_priority_fee_wei(1_500_000_000) // 1.5 gwei
    .with_explorer("https://sepolia.etherscan.io")
}

/// Polygon mainnet.
pub fn polygon_mainnet() -> EvmNetworkConfig {
    EvmNetworkConfig::new(
        "polygon",
        "Polygon Mainnet",
        137,
        "https://polygon-bor-rpc.publicnode.com",
        "MATIC",
        "Polygon",
        false,
    )
    .with_fallback_rpcs(&["https://polygon-rpc.com"])
    // Polygon tips are typically tens of gwei; 1.5 gwei would under-tip.
    .with_priority_fee_wei(30_000_000_000) // 30 gwei
    .with_explorer("https://polygonscan.com")
}

/// BSC mainnet.
pub fn bsc_mainnet() -> EvmNetworkConfig {
    EvmNetworkConfig::new(
        "bsc",
        "BSC Mainnet",
        56,
        "https://bsc-dataseed.binance.org",
        "BNB",
        "Binance Coin",
        false,
    )
    .with_fallback_rpcs(&[
        "https://bsc-dataseed1.binance.org",
        "https://bsc-dataseed2.binance.org",
        "https://bsc-dataseed3.binance.org",
    ])
    .with_priority_fee_wei(1_000_000_000) // 1 gwei
    .with_explorer("https://bscscan.com")
}

/// Base mainnet.
pub fn base_mainnet() -> EvmNetworkConfig {
    EvmNetworkConfig::new(
        "base",
        "Base Mainnet",
        8453,
        "https://mainnet.base.org",
        "ETH",
        "Ethereum",
        false,
    )
    .with_fallback_rpcs(&["https://base-rpc.publicnode.com"])
    // L2 tips are near-zero; 1.5 gwei would overpay.
    .with_priority_fee_wei(1_000_000) // 0.001 gwei
    .with_explorer("https://basescan.org")
}

/// All built-in EVM networks, PulseChain first.
pub fn builtin_networks() -> Vec<EvmNetworkConfig> {
    vec![
        pulsechain_mainnet(),
        pulsechain_testnet_v4(),
        ethereum_mainnet(),
        ethereum_sepolia(),
        polygon_mainnet(),
        bsc_mainnet(),
        base_mainnet(),
    ]
}

/// Find a built-in network by chain id.
pub fn get_network_by_chain_id(chain_id: u64) -> Option<EvmNetworkConfig> {
    builtin_networks()
        .into_iter()
        .find(|n| n.chain_id == chain_id)
}

/// Resolve `wallet_switchEthereumChain` after hex/decimal quantity parsing.
///
/// Some PulseChain dApps (Switch.win) send `chainId: "0x369"` meaning decimal
/// **369**, not the correct EIP-155 hex `0x171`. That mis-encoding becomes
/// decimal **873** after hex parsing — alias it back to PulseChain (369).
pub fn resolve_switch_chain_id(decimal_id: u64) -> Option<EvmNetworkConfig> {
    if let Some(net) = get_network_by_chain_id(decimal_id) {
        return Some(net);
    }
    if decimal_id == 873 {
        return get_network_by_chain_id(369);
    }
    None
}

/// Find a built-in network by id (case-insensitive).
pub fn get_network_by_id(id: &str) -> Option<EvmNetworkConfig> {
    let needle = id.trim().to_ascii_lowercase();
    builtin_networks()
        .into_iter()
        .find(|n| n.id.to_ascii_lowercase() == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_networks_has_pulsechain_first() {
        let nets = builtin_networks();
        assert_eq!(nets.len(), 7);
        assert_eq!(nets[0].id, "pulsechain");
    }

    #[test]
    fn lookup_by_chain_id_and_id() {
        assert_eq!(
            get_network_by_chain_id(369).unwrap().name,
            "PulseChain Mainnet"
        );
        assert!(get_network_by_chain_id(943).unwrap().is_testnet);
        assert_eq!(get_network_by_id("ETHEREUM").unwrap().chain_id, 1);
        assert!(get_network_by_id("does-not-exist").is_none());
    }

    #[test]
    fn resolve_switch_chain_id_aliases_pulsechain_0x369_quirk() {
        assert_eq!(
            resolve_switch_chain_id(873).unwrap().chain_id,
            369,
            "0x369 hex mis-encoding → decimal 873 should map to PulseChain 369"
        );
        assert_eq!(resolve_switch_chain_id(369).unwrap().chain_id, 369);
        assert!(resolve_switch_chain_id(1).unwrap().chain_id == 1);
        assert!(resolve_switch_chain_id(999_999).is_none());
    }

    #[test]
    fn pulsechain_has_cheap_priority_fee_and_fallbacks() {
        let net = pulsechain_mainnet();
        // Audit 4.2: 1.5 gwei would overpay on PulseChain's sub-gwei market.
        assert_eq!(net.default_priority_fee_wei, Some(10_000_000));
        assert!(!net.fallback_rpc_urls.is_empty());
        assert!(net
            .fallback_rpc_urls
            .iter()
            .any(|u| u.contains("publicnode")));
    }

    #[test]
    fn resolve_rpc_endpoints_user_primary_with_fallbacks() {
        let net = pulsechain_mainnet();
        let (primary, fallbacks) =
            resolve_rpc_endpoints(&net, Some("https://rpc-pulsechain.g4mm4.io"), None);
        assert_eq!(primary, "https://rpc-pulsechain.g4mm4.io");
        assert!(fallbacks.contains(&"https://rpc.pulsechain.com".to_string()));
        assert!(fallbacks.contains(&"https://pulsechain-rpc.publicnode.com".to_string()));
        assert!(!fallbacks.contains(&primary));
    }

    #[test]
    fn known_rpc_endpoints_lists_primary_and_fallbacks() {
        let net = pulsechain_mainnet();
        let eps = net.known_rpc_endpoints();
        assert_eq!(eps.len(), 6);
        assert_eq!(eps[0].label, "Official");
        assert!(eps.iter().any(|e| e.url == "https://rpc.pulsechainrpc.com"));
        assert!(eps.iter().any(|e| e.url == "https://rpc.gigatheminter.com"));
    }
}
