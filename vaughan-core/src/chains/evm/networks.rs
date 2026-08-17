//! Built-in EVM network configurations.
//!
//! PulseChain is the primary target; Ethereum and other common EVM chains are
//! included so the wallet is multi-chain ready out of the box.

use serde::{Deserialize, Serialize};

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
    /// Block explorer base URL.
    pub explorer_url: Option<String>,
    /// Native token symbol (e.g. ETH, PLS).
    pub native_symbol: String,
    /// Native token name.
    pub native_name: String,
    /// Native token decimals (18 for most EVM chains).
    pub decimals: u8,
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
            explorer_url: None,
            native_symbol: native_symbol.into(),
            native_name: native_name.into(),
            decimals: 18,
            is_testnet,
        }
    }

    pub fn with_explorer(mut self, explorer_url: impl Into<String>) -> Self {
        self.explorer_url = Some(explorer_url.into());
        self
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
    .with_explorer("https://etherscan.io")
}

/// PulseChain mainnet (chain id 369).
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
}
