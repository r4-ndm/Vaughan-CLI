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

use bip39::Mnemonic;
use secrecy::SecretString;
use zeroize::Zeroize;

use crate::chains::evm::networks::EvmNetworkConfig;
use crate::chains::evm::EvmAdapter;
use crate::chains::{Balance, ChainAdapter, Fee, TxHash};
use crate::core::account::AccountManager;
use crate::core::network::NetworkService;
use crate::core::persistence::{PersistedState, StateManager};
use crate::core::transaction::TransactionService;
use crate::error::WalletError;
use crate::security::encryption::{decrypt, encrypt};
use crate::security::hd_wallet::validate_mnemonic;

/// Top-level wallet state.
pub struct WalletState {
    state: StateManager,
    networks: NetworkService,
    persisted: Option<PersistedState>,
    accounts: Option<AccountManager>,
}

impl WalletState {
    /// Load (or discover) the wallet at `path`.
    pub fn load(path: PathBuf) -> Result<Self, WalletError> {
        let state = StateManager::new(path);
        let persisted = match state.load() {
            Ok(persisted) => persisted,
            Err(WalletError::NotInitialized) => {
                return Ok(Self {
                    state,
                    networks: NetworkService::default(),
                    persisted: None,
                    accounts: None,
                });
            }
            Err(e) => return Err(e),
        };
        let networks = NetworkService::new(&persisted.active_network_id)?;
        Ok(Self {
            state,
            networks,
            persisted: Some(persisted),
            accounts: None,
        })
    }

    /// The vault file path.
    pub fn path(&self) -> &Path {
        self.state.path()
    }

    /// The network service (listing + active selection).
    pub fn networks(&self) -> &NetworkService {
        &self.networks
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

        let persisted = PersistedState::new(vault, self.networks.active_id());
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

    /// Native balance of the active account on the active network.
    pub async fn balance(&self) -> Result<Balance, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = EvmAdapter::new(&net.rpc_url, net.chain_id, &net.name).await?;
        adapter.get_balance(address).await
    }

    /// Estimate the fee to send `value_wei` (base units) to `to`.
    pub async fn estimate_fee(&self, to: &str, value_wei: &str) -> Result<Fee, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = EvmAdapter::new(&net.rpc_url, net.chain_id, &net.name).await?;
        let service = TransactionService::new();
        let tx = service.build_native_transfer(address, to, value_wei, net.chain_id)?;
        service.estimate_fee(&adapter, &tx).await
    }

    /// Build, estimate, sign, and broadcast a native transfer. The caller (UI)
    /// must have shown the user the fee and obtained explicit approval first.
    pub async fn send(&self, to: &str, value_wei: &str) -> Result<TxHash, WalletError> {
        let accounts = self.require_unlocked()?;
        let net = self.networks.active();
        let signer = accounts.active_signer()?;
        let adapter =
            EvmAdapter::with_signer(&net.rpc_url, net.chain_id, &net.name, signer).await?;
        let service = TransactionService::new();
        let mut tx = service.build_native_transfer(
            accounts.active_address(),
            to,
            value_wei,
            net.chain_id,
        )?;
        let fee = adapter.estimate_fee(&tx).await?;
        service.apply_fee(&mut tx, &fee)?;
        service.send(&adapter, tx).await
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
}
