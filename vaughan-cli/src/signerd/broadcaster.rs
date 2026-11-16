//! Broadcaster abstraction for live signing and test harness execution.
//!
//! Single purpose: the [`TransactionBroadcaster`] trait (`broadcast`, `sweep_stealth`,
//! `quote_fee`, `broadcast_phased`), its live `WalletBroadcaster` and test `MockBroadcaster`
//! implementations. Must NOT contain gas-ceiling reservation or ledger logic — that is
//! [`super::gas::Signer::broadcast_metered`]'s job; this module only prices and sends.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::WalletState;

use super::gas::SendPhase;

/// A priced transaction: `tx` carries `gas_limit`/`max_fee_per_gas`/`max_priority_fee_per_gas`
/// all `Some` (validated), and `max_fee_wei` is the worst-case total fee
/// (`gas_limit.saturating_mul(max_fee_per_gas)`) booked against the session gas ceiling.
/// The tx that is quoted here is byte-for-byte the tx that is signed: `broadcast_phased`
/// must be called with `quote.tx`, never a re-derived one.
#[derive(Debug, Clone)]
pub struct FeeQuote {
    pub tx: EvmTransaction,
    pub max_fee_wei: U256,
}

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

    /// Price `tx` without signing or sending it. A quote built from RPC data must be sane:
    /// implementations validate `gas_limit >= 21_000` and `max_fee_per_gas` present and > 0
    /// (a zero/absent fee must never read as "free") before returning `Ok`.
    async fn quote_fee(&self, tx: EvmTransaction) -> Result<FeeQuote, String>;

    /// Like [`Self::broadcast`], but on failure also reports a [`SendPhase`]. Default: every
    /// `Err` from [`Self::broadcast`] is conservatively reported `Uncertain` (an implementation
    /// that cannot tell signing/fee errors apart from node rejections must assume money may
    /// have moved). [`WalletBroadcaster`] overrides this with a real classification.
    async fn broadcast_phased(&self, tx: EvmTransaction) -> Result<String, (SendPhase, String)> {
        self.broadcast(tx)
            .await
            .map_err(|e| (SendPhase::Uncertain, e))
    }
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

/// Validate a fee-filled tx before it is trusted as a quote (see [`TransactionBroadcaster::quote_fee`]).
fn validate_quote(tx: EvmTransaction) -> Result<FeeQuote, String> {
    let gas_limit = tx
        .gas_limit
        .ok_or_else(|| "gas_unavailable: quote missing gas_limit".to_string())?;
    if gas_limit < 21_000 {
        return Err(format!(
            "gas_unavailable: quoted gas_limit {gas_limit} is below the 21000 floor"
        ));
    }
    let max_fee_str = tx
        .max_fee_per_gas
        .as_deref()
        .ok_or_else(|| "gas_unavailable: quote missing max_fee_per_gas".to_string())?;
    let max_fee_per_gas = U256::from_str(max_fee_str).map_err(|e| {
        format!("gas_unavailable: quoted max_fee_per_gas {max_fee_str:?} is not a valid U256: {e}")
    })?;
    if max_fee_per_gas.is_zero() {
        return Err("gas_unavailable: quoted max_fee_per_gas is zero".to_string());
    }
    let max_fee_wei = U256::from(gas_limit).saturating_mul(max_fee_per_gas);
    Ok(FeeQuote { tx, max_fee_wei })
}

/// Map the core broadcast-phase classification to this crate's [`SendPhase`].
fn map_phase(phase: vaughan_core::core::broadcasts::BroadcastPhase) -> SendPhase {
    match phase {
        vaughan_core::core::broadcasts::BroadcastPhase::Definitive => SendPhase::Definitive,
        vaughan_core::core::broadcasts::BroadcastPhase::Uncertain => SendPhase::Uncertain,
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

    async fn quote_fee(&self, tx: EvmTransaction) -> Result<FeeQuote, String> {
        let w = self.wallet.lock().await;
        let filled = w.fill_fee(tx).await.map_err(|e| e.user_message())?;
        validate_quote(filled)
    }

    async fn broadcast_phased(&self, tx: EvmTransaction) -> Result<String, (SendPhase, String)> {
        let w = self.wallet.lock().await;
        match w.broadcast_phased(tx, "sentient").await {
            Ok(receipt) => Ok(receipt.hash),
            Err((phase, e)) => Err((map_phase(phase), e.user_message())),
        }
    }
}

/// In-memory mock broadcaster for testing.
#[derive(Debug)]
pub struct MockBroadcaster {
    pub last_tx: Arc<Mutex<Option<EvmTransaction>>>,
    pub should_fail: Arc<Mutex<bool>>,
    pub fail_message: Arc<Mutex<String>>,
    pub next_tx_hash: Arc<Mutex<String>>,
    quote_gas_limit: Arc<Mutex<u64>>,
    quote_max_fee_per_gas: Arc<Mutex<U256>>,
    quote_fail: Arc<Mutex<Option<String>>>,
    fail_phase: Arc<Mutex<SendPhase>>,
    /// Test 9 (trip race): when armed, `quote_fee` blocks on this until released, so a test
    /// can land a trip while a quote is in flight.
    quote_gate_armed: Arc<Mutex<bool>>,
    quote_gate: Arc<tokio::sync::Notify>,
}

impl Default for MockBroadcaster {
    fn default() -> Self {
        Self::new()
    }
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
            // Small and fixed so tests that don't care about gas keep their existing meaning:
            // 21000 * 1 gwei = 2.1e13 wei, far under the default 5e16 wei ceiling.
            quote_gas_limit: Arc::new(Mutex::new(21_000)),
            quote_max_fee_per_gas: Arc::new(Mutex::new(U256::from(1_000_000_000u64))),
            quote_fail: Arc::new(Mutex::new(None)),
            fail_phase: Arc::new(Mutex::new(SendPhase::Definitive)),
            quote_gate_armed: Arc::new(Mutex::new(false)),
            quote_gate: Arc::new(tokio::sync::Notify::new()),
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

    /// Set the quoted `max_fee_per_gas` (wei/gas) returned by `quote_fee`.
    pub fn set_quote_max_fee(&self, max_fee_per_gas: U256) {
        *self.quote_max_fee_per_gas.lock().unwrap() = max_fee_per_gas;
    }

    /// Set the quoted `gas_limit` returned by `quote_fee`.
    pub fn set_quote_gas_limit(&self, gas_limit: u64) {
        *self.quote_gas_limit.lock().unwrap() = gas_limit;
    }

    /// Make `quote_fee` fail with `msg` (fail-closed: nothing is reserved or sent).
    pub fn set_quote_fail(&self, fail: bool, msg: impl Into<String>) {
        *self.quote_fail.lock().unwrap() = fail.then(|| msg.into());
    }

    /// Classify the next `broadcast`/`broadcast_phased` failure as `Definitive` or `Uncertain`.
    pub fn set_fail_phase(&self, phase: SendPhase) {
        *self.fail_phase.lock().unwrap() = phase;
    }

    /// Arm the quote gate: the next `quote_fee` call blocks until [`Self::release_quote_gate`].
    pub fn arm_quote_gate(&self) {
        *self.quote_gate_armed.lock().unwrap() = true;
    }

    /// Release a `quote_fee` call parked by [`Self::arm_quote_gate`].
    pub fn release_quote_gate(&self) {
        *self.quote_gate_armed.lock().unwrap() = false;
        self.quote_gate.notify_one();
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

    async fn quote_fee(&self, tx: EvmTransaction) -> Result<FeeQuote, String> {
        if *self.quote_gate_armed.lock().unwrap() {
            self.quote_gate.notified().await;
        }
        if let Some(msg) = self.quote_fail.lock().unwrap().clone() {
            return Err(msg);
        }
        let gas_limit = *self.quote_gas_limit.lock().unwrap();
        let max_fee_per_gas = *self.quote_max_fee_per_gas.lock().unwrap();
        let mut quoted = tx;
        quoted.gas_limit = Some(gas_limit);
        quoted.max_fee_per_gas = Some(max_fee_per_gas.to_string());
        quoted.max_priority_fee_per_gas = Some(max_fee_per_gas.to_string());
        validate_quote(quoted)
    }

    async fn broadcast_phased(&self, tx: EvmTransaction) -> Result<String, (SendPhase, String)> {
        match self.broadcast(tx).await {
            Ok(hash) => Ok(hash),
            Err(e) => Err((*self.fail_phase.lock().unwrap(), e)),
        }
    }
}
