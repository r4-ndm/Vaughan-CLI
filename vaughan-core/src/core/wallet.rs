//! Top-level wallet state: the lock/unlock lifecycle plus the operations the
//! UI drives (balance, fee estimate, send, network/account switching).
//!
//! `WalletState` is a thin orchestrator; it owns no logic itself. Accounts are
//! handled by [`AccountManager`], networks by [`NetworkService`], disk I/O by
//! [`StateManager`], and transactions by [`TransactionService`]. It wires them
//! together and enforces the locked/unlocked invariant.
//!
//! # Invariant
//!
//! - `persisted.is_none() && accounts.is_none()` — uninitialized (no vault on disk)
//! - `persisted.is_some() && accounts.is_none()` — initialized but locked
//! - `persisted.is_some() && accounts.is_some()` — unlocked
//!
//! The mnemonic only ever lives inside [`AccountManager`], which zeroizes it on
//! drop; this type deliberately implements no `Debug`/`Display`.

use std::path::{Path, PathBuf};

use alloy::signers::local::PrivateKeySigner;
use bip39::Mnemonic;
use secrecy::SecretString;
use zeroize::Zeroize;

use crate::chains::evm::networks::EvmNetworkConfig;
use crate::chains::evm::EvmAdapter;
use crate::chains::{Balance, ChainAdapter, ChainTransaction, EvmTransaction, Fee, TxHash};
use crate::core::account::AccountManager;
use crate::core::network::NetworkService;
use crate::core::persistence::{PersistedState, StateManager, DEFAULT_PROFILE};
use crate::core::profile::OperatingMode;
use crate::core::transaction::TransactionService;
use crate::error::WalletError;
use crate::security::encryption::{decrypt, encrypt};
use crate::security::hd_wallet::validate_mnemonic;

/// Top-level wallet state.
pub struct WalletState {
    state: StateManager,
    networks: NetworkService,
    /// Optional RPC override applied to the active network (CLI `--rpc-url`,
    /// testnet/dev nodes). Never persisted.
    rpc_override: Option<String>,
    persisted: Option<PersistedState>,
    accounts: Option<AccountManager>,
    /// Session-locked operating mode.
    session_mode: OperatingMode,
    /// Active profile name (e.g. "default", "degen").
    session_profile: String,
}

impl WalletState {
    /// Load (or discover) the wallet at `path` with default session settings.
    pub fn load(path: PathBuf) -> Result<Self, WalletError> {
        Self::load_with_session(path, OperatingMode::HumanOnly, DEFAULT_PROFILE)
    }

    /// Load (or discover) the wallet at `path` with an explicit session operating mode and profile.
    pub fn load_with_session(
        path: PathBuf,
        mode: OperatingMode,
        profile: impl Into<String>,
    ) -> Result<Self, WalletError> {
        let profile_str = profile.into();
        let state = StateManager::new(path);
        let persisted = match state.load() {
            Ok(persisted) => persisted,
            Err(WalletError::NotInitialized) => {
                return Ok(Self {
                    state,
                    networks: NetworkService::default(),
                    rpc_override: None,
                    persisted: None,
                    accounts: None,
                    session_mode: mode,
                    session_profile: profile_str,
                });
            }
            Err(e) => return Err(e),
        };
        let networks = NetworkService::new(&persisted.active_network_id)?;
        let effective_mode = if mode != OperatingMode::HumanOnly {
            mode
        } else {
            persisted.operating_mode
        };
        let effective_profile = if profile_str != DEFAULT_PROFILE {
            profile_str
        } else {
            persisted.profile_name.clone()
        };
        Ok(Self {
            state,
            networks,
            rpc_override: None,
            persisted: Some(persisted),
            accounts: None,
            session_mode: effective_mode,
            session_profile: effective_profile,
        })
    }

    /// Load wallet state for a named profile (e.g. "default", "degen").
    pub fn load_profile(profile_name: &str, mode: OperatingMode) -> Result<Self, WalletError> {
        let path = StateManager::profile_path(profile_name)?;
        Self::load_with_session(path, mode, profile_name)
    }

    /// The active operating mode for this session.
    pub fn operating_mode(&self) -> OperatingMode {
        self.session_mode
    }

    /// Set operating mode (used at welcome screen before session lock).
    pub fn set_operating_mode(&mut self, mode: OperatingMode) {
        self.session_mode = mode;
        if let Some(ref mut p) = self.persisted {
            p.operating_mode = mode;
        }
    }

    /// The active profile name.
    pub fn profile_name(&self) -> &str {
        &self.session_profile
    }

    /// The vault file path.
    pub fn path(&self) -> &Path {
        self.state.path()
    }

    /// The network service (listing + active selection).
    pub fn networks(&self) -> &NetworkService {
        &self.networks
    }

    /// Override the RPC endpoint used for the active network (not persisted;
    /// for `--rpc-url`, dev nodes, or a dedicated provider).
    pub fn set_rpc_override(&mut self, url: impl Into<String>) {
        self.rpc_override = Some(url.into());
    }

    /// The effective RPC url for the active network (override wins).
    fn effective_rpc(&self) -> String {
        self.rpc_override
            .clone()
            .unwrap_or_else(|| self.networks.active().rpc_url.clone())
    }

    /// True once a vault exists on disk.
    pub fn is_initialized(&self) -> bool {
        self.persisted.is_some()
    }

    /// True while the mnemonic is decrypted in memory.
    pub fn is_unlocked(&self) -> bool {
        self.accounts.is_some()
    }

    /// The active account address (requires an unlocked wallet).
    pub fn active_address(&self) -> Result<&str, WalletError> {
        Ok(self.require_unlocked()?.active_address())
    }

    /// The active account's signing key (requires an unlocked wallet).
    ///
    /// Exposed for flows that sign outside the built-in send path (e.g. the
    /// AA batched-send view); the caller drops the key when done.
    pub fn active_signer(&self) -> Result<PrivateKeySigner, WalletError> {
        self.require_unlocked()?.active_signer()
    }

    /// The effective RPC URL of the active network (override if set).
    pub fn active_rpc_url(&self) -> String {
        self.effective_rpc()
    }

    // ---- onboarding ----

    /// Create a brand-new wallet from `mnemonic` and persist it under `password`.
    pub fn create(
        &mut self,
        password: &SecretString,
        mnemonic: Mnemonic,
    ) -> Result<(), WalletError> {
        if self.is_initialized() {
            return Err(WalletError::Other(
                "a wallet already exists at this path".to_string(),
            ));
        }
        // Encrypt the phrase; the transient String is zeroized immediately after.
        let mut phrase = mnemonic.to_string();
        let vault = encrypt(phrase.as_bytes(), password)?;
        phrase.zeroize();

        let persisted = PersistedState::with_mode_and_profile(
            vault,
            self.networks.active_id(),
            self.session_mode,
            &self.session_profile,
        );
        self.state.save(&persisted)?;
        let accounts = AccountManager::new(mnemonic, AccountManager::DEFAULT_ACCOUNT_COUNT)?;
        self.persisted = Some(persisted);
        self.accounts = Some(accounts);
        Ok(())
    }

    /// Restore a wallet from a mnemonic phrase (validated before storing).
    pub fn restore(&mut self, password: &SecretString, phrase: &str) -> Result<(), WalletError> {
        let mnemonic = validate_mnemonic(phrase)?;
        self.create(password, mnemonic)
    }

    // ---- lock / unlock ----

    /// Unlock: decrypt the vault in memory and derive the account list.
    pub fn unlock(&mut self, password: &SecretString) -> Result<(), WalletError> {
        if self.is_unlocked() {
            return Ok(());
        }
        let persisted = self.persisted.as_ref().ok_or(WalletError::NotInitialized)?;
        let mut plaintext = decrypt(&persisted.vault, password)?;
        let phrase = std::str::from_utf8(&plaintext).map_err(|_| {
            WalletError::DecryptionFailed("vault did not contain a valid mnemonic".to_string())
        })?;
        let mut accounts =
            AccountManager::from_phrase(phrase, AccountManager::DEFAULT_ACCOUNT_COUNT)?;
        accounts.set_active(persisted.active_account_index)?;
        plaintext.zeroize();
        self.accounts = Some(accounts);
        Ok(())
    }

    /// Lock: drop the in-memory mnemonic (zeroized on drop).
    pub fn lock(&mut self) {
        self.accounts = None;
    }

    // ---- settings ----

    /// Switch the active network and persist the selection.
    pub fn set_active_network(&mut self, id: &str) -> Result<(), WalletError> {
        self.networks.set_active(id)?;
        if let Some(persisted) = self.persisted.as_mut() {
            persisted.active_network_id = self.networks.active_id().to_string();
            self.state.save(persisted)?;
        }
        Ok(())
    }

    /// Switch the active account and persist the selection.
    pub fn set_active_account(&mut self, index: u32) -> Result<(), WalletError> {
        let accounts = self.accounts.as_mut().ok_or(WalletError::WalletLocked)?;
        accounts.set_active(index)?;
        if let Some(persisted) = self.persisted.as_mut() {
            persisted.active_account_index = index;
            self.state.save(persisted)?;
        }
        Ok(())
    }

    // ---- unlocked operations ----

    /// Create an `EvmAdapter` for the active network.
    pub async fn active_adapter(&self) -> Result<EvmAdapter, WalletError> {
        let net = self.networks.active();
        EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await
    }

    /// Native balance of the active account on the active network.
    pub async fn balance(&self) -> Result<Balance, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
        adapter.get_balance(address).await
    }

    /// All detected balances of the active account: the native asset plus
    /// every curated per-chain ERC-20 (auto asset detection).
    ///
    /// ERC-20s are read in one Multicall3 `tryAggregate` batch (EIP-20 +
    /// mds1/multicall; see `docs/optimizations.md` for provenance), with a
    /// sequential fallback when Multicall3 is absent. Zero balances are
    /// excluded; symbol/decimals come from the contract (cached), falling
    /// back to the curated registry.
    pub async fn assets(&self) -> Result<Vec<Balance>, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
        adapter.get_assets(address).await
    }

    /// Balance of a single ERC-20 (`token_address`) for the active account.
    pub async fn token_balance(&self, token_address: &str) -> Result<Balance, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
        adapter.get_token_balance(token_address, address).await
    }

    /// Estimate the fee to send `value_wei` (base units) to `to`.
    pub async fn estimate_fee(&self, to: &str, value_wei: &str) -> Result<Fee, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
        let service = TransactionService::new();
        let tx = service.build_native_transfer(address, to, value_wei, net.chain_id)?;
        service.estimate_fee(&adapter, &tx).await
    }

    /// Build, estimate, sign, and broadcast a native transfer. The caller (UI)
    /// must have shown the user the fee and obtained explicit approval first.
    pub async fn send(&self, to: &str, value_wei: &str) -> Result<TxHash, WalletError> {
        let accounts = self.require_unlocked()?;
        let net = self.networks.active();
        let tx = TransactionService::new().build_native_transfer(
            accounts.active_address(),
            to,
            value_wei,
            net.chain_id,
        )?;
        let ChainTransaction::Evm(evm_tx) = tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".to_string(),
            ));
        };
        self.send_transaction(evm_tx).await
    }

    /// Build, estimate, sign, and broadcast an arbitrary EVM transaction
    /// (native transfer or contract call, with optional `data`). Missing
    /// gas/fee parameters are filled from a fee estimate. The caller must have
    /// shown the user the request and obtained explicit approval first.
    pub async fn send_transaction(&self, tx: EvmTransaction) -> Result<TxHash, WalletError> {
        let (adapter, tx) = self.signed_adapter_and_tx(tx).await?;
        let service = TransactionService::new();
        service.send(&adapter, ChainTransaction::Evm(tx)).await
    }

    /// Sign an EVM transaction without broadcasting it; returns the raw signed
    /// tx as `0x`-prefixed hex (serves `vaughan_signTransaction`).
    pub async fn sign_transaction(&self, tx: EvmTransaction) -> Result<String, WalletError> {
        let (adapter, tx) = self.signed_adapter_and_tx(tx).await?;
        adapter.sign_transaction(ChainTransaction::Evm(tx)).await
    }

    /// Estimate the fee for an arbitrary EVM transaction payload.
    ///
    /// Used by provider approval UX to show the user a fee before signing.
    pub async fn estimate_transaction_fee(&self, tx: EvmTransaction) -> Result<Fee, WalletError> {
        self.require_unlocked()?;
        let net = self.networks.active();
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
        adapter.estimate_fee(&ChainTransaction::Evm(tx)).await
    }

    /// Sign `message` as an EIP-191 personal message with the active account;
    /// returns the signature as a `0x`-prefixed hex string.
    pub fn sign_message(&self, message: &[u8]) -> Result<String, WalletError> {
        let signer = self.require_unlocked()?.active_signer()?;
        crate::security::signing::sign_personal_message(&signer, message)
    }

    /// Sign an EIP-712 typed-data payload with the active account; returns the
    /// signature as a `0x`-prefixed hex string.
    pub fn sign_typed_data(&self, typed_data: &serde_json::Value) -> Result<String, WalletError> {
        let signer = self.require_unlocked()?.active_signer()?;
        crate::security::signing::sign_typed_data(&signer, typed_data)
    }

    /// Build a signer-backed adapter for the active network and prepare `tx`
    /// (fill missing gas/fees) for signing or broadcast.
    async fn signed_adapter_and_tx(
        &self,
        mut tx: EvmTransaction,
    ) -> Result<(EvmAdapter, EvmTransaction), WalletError> {
        let accounts = self.require_unlocked()?;
        let net = self.networks.active();
        let signer = accounts.active_signer()?;
        let adapter = EvmAdapter::with_signer(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            signer,
            &net.fallback_rpc_urls,
        )
        .await?;
        // Only estimate when the caller left gas/fees unspecified; a
        // fully-specified tx (e.g. from the browser signer backend) is signed
        // exactly as given.
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
            tx = prepared;
        }
        Ok((adapter, tx))
    }

    // ---- helpers ----

    /// The active network config + active address (requires unlocked).
    fn active_context(&self) -> Result<(&EvmNetworkConfig, &str), WalletError> {
        let accounts = self.require_unlocked()?;
        Ok((self.networks.active(), accounts.active_address()))
    }

    /// The unlocked account manager, or a clear error.
    fn require_unlocked(&self) -> Result<&AccountManager, WalletError> {
        if !self.is_initialized() {
            return Err(WalletError::NotInitialized);
        }
        self.accounts.as_ref().ok_or(WalletError::WalletLocked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const TEST_ADDRESS_0: &str = "0x9858effd232b4033e47d90003d41ec34ecaeda94";

    fn password() -> SecretString {
        SecretString::from("CorrectHorse9!BatteryStaple".to_string())
    }

    fn mnemonic() -> Mnemonic {
        validate_mnemonic(TEST_MNEMONIC).unwrap()
    }

    fn tmp_path() -> PathBuf {
        tempfile::tempdir().unwrap().path().join("wallet.json")
    }

    #[test]
    fn load_uninitialized_when_no_file() {
        let w = WalletState::load(tmp_path()).unwrap();
        assert!(!w.is_initialized());
        assert!(!w.is_unlocked());
    }

    #[test]
    fn create_unlocks_and_persists() {
        let mut w = WalletState::load(tmp_path()).unwrap();
        w.create(&password(), mnemonic()).unwrap();
        assert!(w.is_initialized());
        assert!(w.is_unlocked());
        assert_eq!(w.active_address().unwrap().to_lowercase(), TEST_ADDRESS_0);
        assert!(w.path().exists());
    }

    #[test]
    fn create_twice_fails() {
        let mut w = WalletState::load(tmp_path()).unwrap();
        w.create(&password(), mnemonic()).unwrap();
        assert!(w.create(&password(), mnemonic()).is_err());
    }

    #[test]
    fn lock_unlock_roundtrip() {
        let mut w = WalletState::load(tmp_path()).unwrap();
        w.create(&password(), mnemonic()).unwrap();
        w.lock();
        assert!(w.is_initialized());
        assert!(!w.is_unlocked());

        let wrong = SecretString::from("WrongPassword9!BatteryStaple".to_string());
        assert!(w.unlock(&wrong).is_err());
        assert!(!w.is_unlocked());

        w.unlock(&password()).unwrap();
        assert!(w.is_unlocked());
        assert_eq!(w.active_address().unwrap().to_lowercase(), TEST_ADDRESS_0);
    }

    #[test]
    fn unlock_persists_across_reload() {
        let path = tmp_path();
        {
            let mut w = WalletState::load(path.clone()).unwrap();
            w.create(&password(), mnemonic()).unwrap();
            w.lock();
        }
        let mut w = WalletState::load(path).unwrap();
        assert!(w.is_initialized());
        assert!(!w.is_unlocked());
        w.unlock(&password()).unwrap();
        assert_eq!(w.active_address().unwrap().to_lowercase(), TEST_ADDRESS_0);
    }

    #[test]
    fn restore_from_phrase() {
        let mut w = WalletState::load(tmp_path()).unwrap();
        w.restore(&password(), TEST_MNEMONIC).unwrap();
        assert_eq!(w.active_address().unwrap().to_lowercase(), TEST_ADDRESS_0);
    }

    #[test]
    fn network_selection_persists() {
        let path = tmp_path();
        {
            let mut w = WalletState::load(path.clone()).unwrap();
            w.create(&password(), mnemonic()).unwrap();
            w.set_active_network("sepolia").unwrap();
        }
        let w = WalletState::load(path).unwrap();
        assert_eq!(w.networks().active_id(), "sepolia");
    }

    #[test]
    fn account_selection_persists() {
        let path = tmp_path();
        {
            let mut w = WalletState::load(path.clone()).unwrap();
            w.create(&password(), mnemonic()).unwrap();
            w.set_active_account(1).unwrap();
        }
        let mut w = WalletState::load(path).unwrap();
        w.unlock(&password()).unwrap();
        assert_eq!(w.require_unlocked().unwrap().active_index(), 1);
    }

    #[test]
    fn locked_wallet_rejects_operations() {
        let mut w = WalletState::load(tmp_path()).unwrap();
        w.create(&password(), mnemonic()).unwrap();
        w.lock();
        assert!(matches!(w.active_address(), Err(WalletError::WalletLocked)));
        assert!(matches!(
            w.require_unlocked(),
            Err(WalletError::WalletLocked)
        ));
    }

    #[test]
    fn sign_message_and_typed_data_use_active_account() {
        let mut w = WalletState::load(tmp_path()).unwrap();
        w.create(&password(), mnemonic()).unwrap();

        let sig = w.sign_message(b"hello").unwrap();
        assert!(sig.starts_with("0x"));
        let bytes = hex::decode(&sig[2..]).unwrap();
        assert_eq!(bytes.len(), 65);
        let signature = alloy::primitives::Signature::from_raw(bytes.as_slice()).unwrap();
        let recovered = signature.recover_address_from_msg(b"hello").unwrap();
        assert_eq!(recovered.to_string().to_lowercase(), TEST_ADDRESS_0);

        let payload = serde_json::json!({
            "types": {
                "EIP712Domain": [],
                "Message": [{"name": "x", "type": "string"}]
            },
            "primaryType": "Message",
            "domain": {},
            "message": {"x": "y"},
        });
        let sig = w.sign_typed_data(&payload).unwrap();
        assert!(sig.starts_with("0x"));
        assert_eq!(hex::decode(&sig[2..]).unwrap().len(), 65);
    }

    #[tokio::test]
    async fn sign_transaction_offline_with_fully_specified_tx() {
        let mut w = WalletState::load(tmp_path()).unwrap();
        w.create(&password(), mnemonic()).unwrap();
        // A fully-specified tx skips fee estimation, so signing needs no RPC.
        let tx = EvmTransaction {
            from: w.active_address().unwrap().to_string(),
            to: "0x0000000000000000000000000000000000000000".to_string(),
            value: "0".to_string(),
            data: Some("0x".to_string()),
            gas_limit: Some(21_000),
            gas_price: None,
            max_fee_per_gas: Some("2000000000".to_string()),
            max_priority_fee_per_gas: Some("1000000000".to_string()),
            nonce: Some(0),
            chain_id: 943,
        };
        let raw = w.sign_transaction(tx).await.unwrap();
        assert!(raw.starts_with("0x"));
        assert!(raw.len() > 4, "signed tx must carry an RLP body");
    }

    #[test]
    fn signing_requires_unlocked_wallet() {
        let w = WalletState::load(tmp_path()).unwrap();
        assert!(matches!(
            w.sign_message(b"x"),
            Err(WalletError::NotInitialized)
        ));
    }
}
