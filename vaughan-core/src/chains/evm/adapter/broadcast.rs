//! `EvmAdapter::broadcast_raw` / `broadcast_raw_phased`: submit an already
//! signed raw envelope and classify a failure by [`BroadcastPhase`] so a
//! caller that reserves funds/gas up front knows whether it is safe to
//! release the reservation. Must NOT contain fee estimation, signing, or
//! nonce logic — those stay in `mod.rs`.

use std::sync::atomic::{AtomicBool, Ordering};

use alloy::providers::Provider;

use crate::chains::TxHash;
use crate::core::broadcasts::BroadcastPhase;
use crate::error::WalletError;

use super::EvmAdapter;

impl EvmAdapter {
    /// Broadcast an already-signed raw transaction (EIP-2718 encoded, no
    /// leading `0x`) through the primary + fallback provider chain.
    ///
    /// Broadcasting the same signed envelope to a fallback endpoint is safe:
    /// identical nonce means a duplicate is a no-op on-chain.
    pub async fn broadcast_raw(&self, raw: Vec<u8>) -> Result<TxHash, WalletError> {
        self.broadcast_raw_phased(raw).await.map_err(|(_, e)| e)
    }

    /// Like [`Self::broadcast_raw`], but on failure also reports whether the raw envelope
    /// definitely never reached the node (safe to treat as unsent) or may have been accepted
    /// before the error surfaced (`Uncertain` — a caller must not re-arm an identical retry).
    /// A JSON-RPC error RESPONSE from the node (nonce too low, underpriced, insufficient
    /// funds, revert-on-submit) is `Definitive`; a transport failure/timeout — the node may
    /// never have seen the request, or may have accepted it and then dropped the connection
    /// before answering — is `Uncertain`.
    ///
    /// `with_provider` retries transport-classified errors against fallback endpoints, so on
    /// multiple attempts this reports the phase of the LAST attempt (same one its returned
    /// error describes).
    pub async fn broadcast_raw_phased(
        &self,
        raw: Vec<u8>,
    ) -> Result<TxHash, (BroadcastPhase, WalletError)> {
        let last_definitive = AtomicBool::new(false);
        let result = self
            .with_provider(|provider| {
                let raw = raw.clone();
                let last_definitive = &last_definitive;
                async move {
                    provider.send_raw_transaction(&raw).await.map_err(|e| {
                        last_definitive.store(e.as_error_resp().is_some(), Ordering::Relaxed);
                        WalletError::TransactionFailed(e.to_string())
                    })
                }
            })
            .await;
        match result {
            Ok(pending) => Ok(TxHash(format!("{:?}", pending.tx_hash()))),
            Err(e) => {
                let phase = if last_definitive.load(Ordering::Relaxed) {
                    BroadcastPhase::Definitive
                } else {
                    BroadcastPhase::Uncertain
                };
                Err((phase, e))
            }
        }
    }
}
