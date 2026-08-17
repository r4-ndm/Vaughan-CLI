//! Network management: the built-in EVM networks and the active selection.
//!
//! The active network id is what gets persisted; the config is looked up on
//! demand so the on-disk state can never drift from the built-in list.

use crate::chains::evm::networks::{builtin_networks, EvmNetworkConfig};
use crate::error::WalletError;

/// Default active network on first run (PulseChain mainnet, FR-1.7).
pub const DEFAULT_NETWORK_ID: &str = "pulsechain";

/// Built-in networks plus the active selection.
#[derive(Debug, Clone)]
pub struct NetworkService {
    networks: Vec<EvmNetworkConfig>,
    active_id: String,
}

impl NetworkService {
    /// Create a service with `active_id` active (must be a built-in network).
    pub fn new(active_id: impl Into<String>) -> Result<Self, WalletError> {
        let networks = builtin_networks();
        let active_id = active_id.into();
        if !networks
            .iter()
            .any(|n| n.id.eq_ignore_ascii_case(&active_id))
        {
            return Err(WalletError::NetworkNotFound(active_id));
        }
        Ok(Self {
            networks,
            active_id,
        })
    }

    /// All built-in networks.
    pub fn networks(&self) -> &[EvmNetworkConfig] {
        &self.networks
    }

    /// The active network config.
    pub fn active(&self) -> &EvmNetworkConfig {
        self.find(&self.active_id)
            .expect("active network id is always valid")
    }

    /// The active network id (the persisted identifier).
    pub fn active_id(&self) -> &str {
        &self.active_id
    }

    /// Look up a network by id (case-insensitive).
    pub fn get(&self, id: &str) -> Option<&EvmNetworkConfig> {
        self.find(id)
    }

    /// Switch the active network (case-insensitive).
    pub fn set_active(&mut self, id: &str) -> Result<(), WalletError> {
        if self.find(id).is_none() {
            return Err(WalletError::NetworkNotFound(id.to_string()));
        }
        self.active_id = id.trim().to_ascii_lowercase();
        Ok(())
    }

    fn find(&self, id: &str) -> Option<&EvmNetworkConfig> {
        self.networks.iter().find(|n| n.id.eq_ignore_ascii_case(id))
    }
}

impl Default for NetworkService {
    fn default() -> Self {
        Self::new(DEFAULT_NETWORK_ID).expect("pulsechain is a built-in network")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_active_is_pulsechain() {
        let ns = NetworkService::default();
        assert_eq!(ns.active_id(), "pulsechain");
        assert_eq!(ns.active().chain_id, 369);
        assert!(!ns.active().is_testnet);
    }

    #[test]
    fn switch_network() {
        let mut ns = NetworkService::default();
        ns.set_active("sepolia").unwrap();
        assert_eq!(ns.active_id(), "sepolia");
        assert_eq!(ns.active().chain_id, 11_155_111);
        assert!(ns.set_active("does-not-exist").is_err());
    }

    #[test]
    fn get_by_id_case_insensitive() {
        let ns = NetworkService::default();
        assert_eq!(ns.get("ETHEREUM").unwrap().chain_id, 1);
        assert!(ns.get("missing").is_none());
    }

    #[test]
    fn new_rejects_unknown_id() {
        assert!(NetworkService::new("nope").is_err());
    }
}
