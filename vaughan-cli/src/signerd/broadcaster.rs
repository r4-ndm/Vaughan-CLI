//! Broadcaster abstraction for live signing and test harness execution.

use alloy::primitives::Address;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::WalletState;

/// Interface for signing and broadcasting transactions.
#[async_trait]
pub trait TransactionBroadcaster: Send + Sync {
    /// Sign and broadcast an EVM transaction, returning the transaction hash.
    async fn broadcast(&self, tx: EvmTransaction) -> Result<String, String>;

    /// Sign and broadcast a stealth sweep transaction.
    async fn sweep_stealth(
        &self,
        chain_id: u64,
        stealth_address: Address,
    ) -> Result<String, String>;
}

/// Live broadcaster signing with an unlocked `WalletState` and broadcasting over RPC.
pub struct WalletBroadcaster {
    wallet: Arc<tokio::sync::Mutex<WalletState>>,
}

impl WalletBroadcaster {
    pub fn new(wallet: Arc<tokio::sync::Mutex<WalletState>>) -> Self {
        Self { wallet }
    }
}

#[async_trait]
impl TransactionBroadcaster for WalletBroadcaster {
    async fn broadcast(&self, tx: EvmTransaction) -> Result<String, String> {
        let w = self.wallet.lock().await;
        let hash = w.send_transaction(tx).await.map_err(|e| e.user_message())?;
        Ok(hash.to_string())
    }

    async fn sweep_stealth(
        &self,
        _chain_id: u64,
        stealth_address: Address,
    ) -> Result<String, String> {
        let w = self.wallet.lock().await;
        let notes = w.scan_stealth_notes().await.map_err(|e| e.user_message())?;
        let note = notes
            .into_iter()
            .find(|n| n.announcement.stealth_address == stealth_address)
            .ok_or_else(|| format!("no unswept note for {stealth_address:#x}"))?;
        let hash = w
            .sweep_stealth_note(&note)
            .await
            .map_err(|e| e.user_message())?;
        Ok(hash.to_string())
    }
}

/// In-memory mock broadcaster for testing.
#[derive(Debug, Default)]
pub struct MockBroadcaster {
    pub last_tx: Arc<Mutex<Option<EvmTransaction>>>,
    pub should_fail: Arc<Mutex<bool>>,
    pub fail_message: Arc<Mutex<String>>,
    pub next_tx_hash: Arc<Mutex<String>>,
}

impl MockBroadcaster {
    pub fn new() -> Self {
        Self {
            last_tx: Arc::new(Mutex::new(None)),
            should_fail: Arc::new(Mutex::new(false)),
            fail_message: Arc::new(Mutex::new("network error: dropped from mempool".into())),
            next_tx_hash: Arc::new(Mutex::new(
                // Obviously-synthetic mock hash: repeated `aa` bytes are on the repo audit's
                // own allowlist, so a fixture can never be mistaken for committed key material.
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            )),
        }
    }

    pub fn set_fail(&self, fail: bool) {
        *self.should_fail.lock().unwrap() = fail;
    }

    pub fn set_fail_message(&self, msg: impl Into<String>) {
        *self.fail_message.lock().unwrap() = msg.into();
    }

    pub fn set_tx_hash(&self, hash: impl Into<String>) {
        *self.next_tx_hash.lock().unwrap() = hash.into();
    }

    pub fn get_last_tx(&self) -> Option<EvmTransaction> {
        self.last_tx.lock().unwrap().clone()
    }
}

#[async_trait]
impl TransactionBroadcaster for MockBroadcaster {
    async fn broadcast(&self, tx: EvmTransaction) -> Result<String, String> {
        *self.last_tx.lock().unwrap() = Some(tx);
        if *self.should_fail.lock().unwrap() {
            return Err(self.fail_message.lock().unwrap().clone());
        }
        Ok(self.next_tx_hash.lock().unwrap().clone())
    }

    async fn sweep_stealth(
        &self,
        _chain_id: u64,
        _stealth_address: Address,
    ) -> Result<String, String> {
        if *self.should_fail.lock().unwrap() {
            return Err(self.fail_message.lock().unwrap().clone());
        }
        Ok(self.next_tx_hash.lock().unwrap().clone())
    }
}
