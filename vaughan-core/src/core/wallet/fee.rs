//! Fee-fill and phased broadcast for a quote that must be the signed tx.
//!
//! `fill_fee` fills missing gas/fee fields from a fresh estimate; `broadcast_phased`
//! classifies a failure by [`BroadcastPhase`]. Shared by the normal sign path
//! (`prepare_sign_raw`/`broadcast`) and the signer daemon's gas-ceiling quote
//! (`WalletBroadcaster::quote_fee`/`broadcast_phased` in vaughan-cli) so the quoted
//! transaction and the signed transaction are byte-for-byte the same tx. Must NOT
//! assign a nonce, sign, or contain per-intent policy logic.

use crate::chains::{ChainAdapter, ChainTransaction, EvmTransaction};
use crate::core::broadcasts::{BroadcastEntry, BroadcastPhase, BroadcastReceipt};
use crate::core::transaction::TransactionService;
use crate::error::WalletError;

use super::WalletState;

impl WalletState {
    /// Fill `gas_limit` / `max_fee_per_gas` / `max_priority_fee_per_gas` from a fresh fee
    /// estimate when either is missing; a tx that already carries both is returned verbatim
    /// (callers like speed-up pre-fill fees deliberately and must not be re-estimated).
    pub async fn fill_fee(&self, tx: EvmTransaction) -> Result<EvmTransaction, WalletError> {
        let net = self.networks.active();
        let adapter = self.adapter_for(net).await?;
        let missing_fees = tx.max_fee_per_gas.is_none() && tx.gas_price.is_none();
        if tx.gas_limit.is_none() || missing_fees {
            let mut chain_tx = ChainTransaction::Evm(tx);
            let fee = adapter.estimate_fee(&chain_tx).await?;
            TransactionService::new().apply_fee(&mut chain_tx, &fee)?;
            let ChainTransaction::Evm(prepared) = chain_tx else {
                return Err(WalletError::InvalidTransaction(
                    "expected an EVM transaction".to_string(),
                ));
            };
            return Ok(prepared);
        }
        Ok(tx)
    }

    /// Like [`Self::broadcast`], but on failure also reports the [`BroadcastPhase`]: errors
    /// raised before the raw envelope is sent (fee fill, nonce, signing) are always
    /// `Definitive`, and [`crate::chains::evm::EvmAdapter::broadcast_raw_phased`] classifies
    /// the network send itself.
    pub async fn broadcast_phased(
        &self,
        tx: EvmTransaction,
        label: &str,
    ) -> Result<BroadcastReceipt, (BroadcastPhase, WalletError)> {
        let (adapter, prepared, raw) = self
            .prepare_sign_raw(tx)
            .await
            .map_err(|e| (BroadcastPhase::Definitive, e))?;
        let hash = adapter.broadcast_raw_phased(raw).await?;
        adapter.invalidate_balance_cache().await;
        let entry = BroadcastEntry::from_prepared(&prepared, hash.0.clone(), label);
        Ok(BroadcastReceipt {
            hash: hash.0,
            entry,
        })
    }
}
