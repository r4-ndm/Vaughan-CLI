//! Chain adapters: a single family-agnostic contract ([`ChainAdapter`]) plus
//! per-family implementations under `chains/{family}/`.
//!
//! ## Adding a new chain family
//!
//! 1. Add a variant to [`ChainType`] and a request variant to [`ChainTransaction`].
//! 2. Create `chains/{family}/` with its own `types.rs`, `networks.rs`, and `adapter.rs`.
//! 3. Implement [`ChainAdapter`] for the new adapter.
//! 4. Register a builder in [`ChainRegistry`].
//!
//! The UI and services talk to a `dyn ChainAdapter` and never match on family.

mod types;
pub use types::*;

pub mod evm;

use async_trait::async_trait;

use crate::error::WalletError;
use evm::{EvmAdapter, EvmNetworkConfig};

/// The single contract every chain family implements.
///
/// Intentionally minimal: family-specific concerns (nonces, coin selection,
/// fee application, address encoding, signing) stay inside each adapter, never
/// in this trait. See [`EvmAdapter`] for an example.
#[async_trait]
pub trait ChainAdapter: Send + Sync {
    /// The chain family.
    fn chain_type(&self) -> ChainType;

    /// Family + network metadata for display.
    fn chain_info(&self) -> ChainInfo;

    /// Validate an address in this family's format.
    fn validate_address(&self, address: &str) -> Result<(), WalletError>;

    /// Native balance for `address`.
    async fn get_balance(&self, address: &str) -> Result<Balance, WalletError>;

    /// Estimate the fee for `tx`.
    async fn estimate_fee(&self, tx: &ChainTransaction) -> Result<Fee, WalletError>;

    /// Sign and broadcast `tx`, returning the transaction id/hash.
    async fn send_transaction(&self, tx: ChainTransaction) -> Result<TxHash, WalletError>;

    /// Look up the status of a previously submitted transaction.
    async fn get_tx_status(&self, tx_hash: &str) -> Result<TxStatus, WalletError>;

    /// Recent native transactions for `address`.
    async fn get_transaction_history(
        &self,
        address: &str,
        limit: u32,
    ) -> Result<Vec<TxRecord>, WalletError>;
}

/// Registry/factory that builds adapters for a chain family + network.
///
/// Only EVM is implemented today. New families add a builder here; callers never
/// match on [`ChainType`].
#[derive(Default)]
pub struct ChainRegistry;

impl ChainRegistry {
    pub fn new() -> Self {
        Self
    }

    /// Build an EVM adapter for `network`.
    pub async fn build_evm(
        &self,
        network: &EvmNetworkConfig,
    ) -> Result<Box<dyn ChainAdapter>, WalletError> {
        let adapter = EvmAdapter::new(
            &network.rpc_url,
            network.chain_id,
            &network.name,
            &network.fallback_rpc_urls,
        )
        .await?;
        Ok(Box::new(adapter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chains::evm::networks::pulsechain_testnet_v4;

    #[tokio::test]
    async fn registry_builds_evm_adapter() {
        let net = pulsechain_testnet_v4();
        let adapter = ChainRegistry::new().build_evm(&net).await.unwrap();
        assert_eq!(adapter.chain_type(), ChainType::Evm);
        assert_eq!(adapter.chain_info().network_id, "943");
        assert!(adapter
            .validate_address("0x0000000000000000000000000000000000000000")
            .is_ok());
        assert!(adapter.validate_address("not-an-address").is_err());
    }
}
