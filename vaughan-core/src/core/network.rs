//! Network management: built-in EVM networks, optional custom nets, active selection.
//!
//! The active network id is persisted; built-ins are looked up from code and
//! customs from the vault so on-disk state cannot invent unknown built-in ids.

use crate::chains::evm::networks::{builtin_networks, EvmNetworkConfig};
use crate::core::persistence::CustomNetwork;
use crate::error::WalletError;

/// Default active network on first run (PulseChain mainnet, FR-1.7).
pub const DEFAULT_NETWORK_ID: &str = "pulsechain";

/// Built-in + custom networks and the active selection.
#[derive(Debug, Clone)]
pub struct NetworkService {
    networks: Vec<EvmNetworkConfig>,
    /// Ids that came from [`CustomNetwork`] (not built-ins).
    custom_ids: Vec<String>,
    active_id: String,
}

impl NetworkService {
    /// Built-ins only, with `active_id` selected.
    pub fn new(active_id: impl Into<String>) -> Result<Self, WalletError> {
        Self::with_custom(active_id, &[])
    }

    /// Built-ins plus user customs. If `active_id` is missing, falls back to PulseChain.
    pub fn with_custom(
        active_id: impl Into<String>,
        custom: &[CustomNetwork],
    ) -> Result<Self, WalletError> {
        let mut networks = builtin_networks();
        let mut custom_ids = Vec::new();
        for c in custom {
            if networks
                .iter()
                .any(|n| n.id.eq_ignore_ascii_case(&c.id) || n.chain_id == c.chain_id)
            {
                // Skip corrupt/conflicting persisted rows rather than fail unlock.
                continue;
            }
            custom_ids.push(c.id.clone());
            networks.push(c.to_evm_config());
        }
        let mut active_id = active_id.into();
        if !networks
            .iter()
            .any(|n| n.id.eq_ignore_ascii_case(&active_id))
        {
            active_id = DEFAULT_NETWORK_ID.to_string();
        }
        Ok(Self {
            networks,
            custom_ids,
            active_id,
        })
    }

    /// Rebuild the list from persisted customs (keeps current active when possible).
    pub fn reload_custom(&mut self, custom: &[CustomNetwork]) -> Result<(), WalletError> {
        let active = self.active_id.clone();
        *self = Self::with_custom(active, custom)?;
        Ok(())
    }

    /// All networks (built-ins then customs).
    pub fn networks(&self) -> &[EvmNetworkConfig] {
        &self.networks
    }

    /// True when `id` is a user-defined network.
    pub fn is_custom(&self, id: &str) -> bool {
        self.custom_ids.iter().any(|c| c.eq_ignore_ascii_case(id))
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
    fn default_is_pulsechain() {
        let ns = NetworkService::default();
        assert_eq!(ns.active_id(), "pulsechain");
        assert_eq!(ns.active().chain_id, 369);
    }

    #[test]
    fn custom_network_appends() {
        let custom = CustomNetwork {
            id: "custom-31337".into(),
            name: "Anvil".into(),
            chain_id: 31337,
            rpc_url: "http://127.0.0.1:8545".into(),
            native_symbol: "ETH".into(),
            is_testnet: true,
        };
        let ns = NetworkService::with_custom("custom-31337", &[custom]).unwrap();
        assert!(ns.is_custom("custom-31337"));
        assert_eq!(ns.active().chain_id, 31337);
    }

    #[test]
    fn rejects_unknown_without_custom_falls_back() {
        let ns = NetworkService::with_custom("nope", &[]).unwrap();
        assert_eq!(ns.active_id(), "pulsechain");
    }
}
