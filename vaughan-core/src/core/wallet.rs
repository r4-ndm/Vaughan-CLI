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
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroize;

use crate::chains::evm::networks::{resolve_rpc_endpoints, EvmNetworkConfig, RpcEndpoint};
use crate::chains::evm::EvmAdapter;
use crate::chains::{
    Balance, ChainAdapter, ChainTransaction, EvmTransaction, Fee, TxHash, TxStatus,
};
use crate::core::account::AccountManager;
use crate::core::network::NetworkService;
use crate::core::persistence::{
    CustomToken, PersistedState, StateManager, TrustedDapp, DEFAULT_PROFILE,
};
use crate::core::profile::OperatingMode;
use crate::core::transaction::TransactionService;
use crate::core::vault_secrets::VaultSecrets;
use crate::error::WalletError;
use crate::security::encryption::{decrypt, encrypt};
use crate::security::hd_wallet::validate_mnemonic;

/// Brief RPC + account context for status-chrome fetches without holding [`WalletState`].
#[derive(Debug, Clone)]
pub struct ChromeRpcSnapshot {
    pub rpc_url: String,
    pub fallback_rpc_urls: Vec<String>,
    pub chain_id: u64,
    pub network_name: String,
    pub address: String,
}

/// Network RPC context for read-only JSON-RPC proxy (works while wallet is locked).
#[derive(Debug, Clone)]
pub struct NetworkRpcSnapshot {
    pub rpc_url: String,
    pub fallback_rpc_urls: Vec<String>,
    pub chain_id: u64,
    pub network_name: String,
}

impl NetworkRpcSnapshot {
    const READ_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    async fn adapter(&self) -> Result<EvmAdapter, WalletError> {
        EvmAdapter::new(
            &self.rpc_url,
            self.chain_id,
            &self.network_name,
            &self.fallback_rpc_urls,
        )
        .await
    }

    /// Forward an allowlisted read method to the active network RPC.
    pub async fn forward_read(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, WalletError> {
        let adapter = self.adapter().await?;
        match tokio::time::timeout(Self::READ_RPC_TIMEOUT, adapter.raw_request(method, params))
            .await
        {
            Ok(r) => r,
            Err(_) => Err(WalletError::NetworkError(
                "read RPC timed out — check RPC / network".into(),
            )),
        }
    }
}

impl ChromeRpcSnapshot {
    const CHROME_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

    async fn adapter(&self) -> Result<EvmAdapter, WalletError> {
        EvmAdapter::new(
            &self.rpc_url,
            self.chain_id,
            &self.network_name,
            &self.fallback_rpc_urls,
        )
        .await
    }

    /// Native balance for the snapshotted account.
    pub async fn balance(&self) -> Result<Balance, WalletError> {
        let adapter = self.adapter().await?;
        match tokio::time::timeout(Self::CHROME_RPC_TIMEOUT, adapter.get_balance(&self.address))
            .await
        {
            Ok(r) => r,
            Err(_) => Err(WalletError::NetworkError(
                "balance RPC timed out — check RPC / network".into(),
            )),
        }
    }

    /// Gas hint in gwei (soft timeout).
    pub async fn gas_price_gwei_display(&self) -> Result<String, WalletError> {
        let adapter = self.adapter().await?;
        match tokio::time::timeout(Self::CHROME_RPC_TIMEOUT, adapter.gas_price_gwei_display()).await
        {
            Ok(r) => r,
            Err(_) => Err(WalletError::NetworkError(
                "gas price RPC timed out — check RPC / network".into(),
            )),
        }
    }

    /// Balance + gas for the TUI chrome strip. Gas failure is soft (`"—"`); balance failure is hard.
    pub async fn fetch_chrome(&self) -> Result<(Balance, String), WalletError> {
        let bal = self.balance().await?;
        let gas = match self.gas_price_gwei_display().await {
            Ok(g) => g,
            Err(_) => "—".into(),
        };
        Ok((bal, gas))
    }
}

enum OwnedSignerBackend {
    Local(crate::security::LocalSignerBackend),
    Ledger(crate::security::LedgerSignerBackend),
    Mock(crate::security::MockSignerBackend),
}

impl OwnedSignerBackend {
    async fn sign(
        &self,
        req: crate::security::SignRequest,
    ) -> Result<crate::security::SignResult, WalletError> {
        use crate::security::SignerBackend;
        match self {
            Self::Local(b) => b.sign(req).await,
            Self::Ledger(b) => b.sign(req).await,
            Self::Mock(b) => b.sign(req).await,
        }
    }
}

fn block_on_wallet<F: std::future::Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("wallet sign runtime");
            rt.block_on(fut)
        }
    }
}

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
    /// Active profile name (e.g. "default", "sentient").
    session_profile: String,
    /// CI / Anvil: sign hardware accounts with this mock instead of USB Ledger.
    hw_mock: Option<crate::security::MockSignerBackend>,
}

/// Everything the expensive half of [`WalletState::unlock`] needs, cloned out of
/// the persisted state so the Argon2id KDF + account derivation can run off the
/// UI thread. Holds only ciphertext and public metadata — no plaintext secrets.
pub struct UnlockPayload {
    vault: crate::security::encryption::EncryptedVault,
    hardware: Vec<crate::security::hardware::HardwareAccountRecord>,
    active_account_index: u32,
}

impl UnlockPayload {
    /// Decrypt the vault and derive the account list (the slow part: Argon2id).
    pub fn decrypt(self, password: &SecretString) -> Result<AccountManager, WalletError> {
        let mut plaintext = decrypt(&self.vault, password)?;
        let phrase = std::str::from_utf8(&plaintext).map_err(|_| {
            WalletError::DecryptionFailed("vault did not contain a valid mnemonic".to_string())
        })?;
        let mut secrets = VaultSecrets::decode(phrase)?;
        plaintext.zeroize();
        let mut accounts = AccountManager::from_secrets_with_hardware(
            &secrets,
            AccountManager::DEFAULT_ACCOUNT_COUNT,
            &self.hardware,
        )?;
        secrets.zeroize();
        if accounts.set_active(self.active_account_index).is_err() {
            // Imported-only edge / stale index: fall back to first account.
            if let Some(first) = accounts.accounts().first().map(|a| a.index) {
                accounts.set_active(first)?;
            }
        }
        Ok(accounts)
    }
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
        let mut persisted = match state.load() {
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
                    hw_mock: None,
                });
            }
            Err(e) => return Err(e),
        };
        if crate::core::persistence::merge_default_trusted_dapps(&mut persisted.trusted_dapps) {
            let _ = state.save(&persisted);
        }
        let networks =
            NetworkService::with_custom(&persisted.active_network_id, &persisted.custom_networks)?;
        if networks.active_id() != persisted.active_network_id {
            persisted.active_network_id = networks.active_id().to_string();
            let _ = state.save(&persisted);
        }
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
            hw_mock: None,
        })
    }

    /// The active operating mode for this session.
    pub fn operating_mode(&self) -> OperatingMode {
        self.session_mode
    }

    /// Set the operating mode for this process session (welcome / unlock picker).
    ///
    /// Also updates the persisted preference so CLI defaults stay in sync; the
    /// TUI always re-prompts at unlock (FR-5.1).
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

    /// Persisted primary RPC override for `network_id`, if any.
    pub fn network_rpc_primary(&self, network_id: &str) -> Option<&str> {
        self.persisted.as_ref().and_then(|p| {
            p.network_rpc_primary
                .get(&network_id.trim().to_ascii_lowercase())
                .map(String::as_str)
        })
    }

    /// Selectable RPC presets for a network (built-in list + current custom primary).
    pub fn known_rpc_endpoints(&self, network_id: &str) -> Vec<RpcEndpoint> {
        let Some(net) = self.networks.get(network_id) else {
            return Vec::new();
        };
        let mut endpoints = net.known_rpc_endpoints();
        if let Some(custom) = self.network_rpc_primary(network_id) {
            if !endpoints.iter().any(|e| e.url == custom) {
                endpoints.insert(
                    0,
                    RpcEndpoint {
                        label: crate::chains::evm::networks::rpc_endpoint_label(custom),
                        url: custom.to_string(),
                    },
                );
            }
        }
        endpoints
    }

    /// Primary + ordered fallback RPC URLs for `net` (respects overrides).
    pub fn rpc_endpoints_for(&self, net: &EvmNetworkConfig) -> (String, Vec<String>) {
        let session_override = if net.id.eq_ignore_ascii_case(self.networks.active_id()) {
            self.rpc_override.as_deref()
        } else {
            None
        };
        resolve_rpc_endpoints(net, self.network_rpc_primary(&net.id), session_override)
    }

    /// Set or clear the persisted primary RPC for a network.
    ///
    /// Built-ins store an override in vault metadata; custom networks update their
    /// base RPC URL directly (chain id is fixed at add time).
    pub fn set_network_rpc_primary(
        &mut self,
        network_id: &str,
        rpc_url: Option<&str>,
    ) -> Result<(), WalletError> {
        let id = network_id.trim().to_ascii_lowercase();
        if self.networks.get(&id).is_none() {
            return Err(WalletError::NetworkNotFound(network_id.to_string()));
        }
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        if self.networks.is_custom(&id) {
            let Some(url) = rpc_url.filter(|u| !u.trim().is_empty()) else {
                return Ok(());
            };
            let url = Self::normalize_rpc_url(url)?;
            let custom = persisted
                .custom_networks
                .iter_mut()
                .find(|n| n.id.eq_ignore_ascii_case(&id))
                .ok_or_else(|| WalletError::NetworkNotFound(id.clone()))?;
            custom.rpc_url = url;
            persisted.network_rpc_primary.remove(&id);
            let customs = persisted.custom_networks.clone();
            self.state.save(persisted)?;
            self.networks.reload_custom(&customs)?;
            return Ok(());
        }
        match rpc_url {
            None | Some("") => {
                persisted.network_rpc_primary.remove(&id);
            }
            Some(url) => {
                let url = Self::normalize_rpc_url(url)?;
                persisted.network_rpc_primary.insert(id, url);
            }
        }
        self.state.save(persisted)?;
        Ok(())
    }

    fn normalize_rpc_url(url: &str) -> Result<String, WalletError> {
        let url = url.trim();
        let parsed = url::Url::parse(url).map_err(|_| {
            WalletError::InvalidTransaction("RPC URL must be a valid http(s) URL".into())
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(WalletError::InvalidTransaction(
                "RPC URL must be http or https".into(),
            ));
        }
        Ok(url.to_string())
    }

    /// The effective RPC url for the active network (override wins).
    fn effective_rpc(&self) -> String {
        self.rpc_endpoints_for(self.networks.active()).0
    }

    /// Build an adapter for `net` using the effective primary + fallbacks.
    pub(crate) async fn adapter_for(
        &self,
        net: &EvmNetworkConfig,
    ) -> Result<EvmAdapter, WalletError> {
        let (primary, fallbacks) = self.rpc_endpoints_for(net);
        EvmAdapter::new(&primary, net.chain_id, &net.name, &fallbacks).await
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

    /// Active account index (HD or imported).
    pub fn active_account_index(&self) -> Result<u32, WalletError> {
        Ok(self.require_unlocked()?.active_index())
    }

    /// Display label for the active account (e.g. `wallet 0` or `W1-HD 1`).
    pub fn active_account_label(&self) -> Result<&str, WalletError> {
        Ok(self.require_unlocked()?.active_account().label.as_str())
    }

    /// Label + address + whether imported, for the F3-active account (Keys UI).
    pub fn active_account_export_context(&self) -> Result<(String, String, bool), WalletError> {
        let a = self.require_unlocked()?.active_account();
        Ok((a.label.clone(), a.address.clone(), a.is_imported))
    }

    /// True when the F3-active account is a hardware watch record.
    pub fn active_is_hardware(&self) -> Result<bool, WalletError> {
        Ok(self.require_unlocked()?.active_account().kind.is_hardware())
    }

    /// Persist a hardware watch account and make it active (no secrets).
    pub fn add_hardware_account(
        &mut self,
        record: crate::security::HardwareAccountRecord,
    ) -> Result<crate::core::account::Account, WalletError> {
        self.require_unlocked()?;
        let account = self
            .accounts
            .as_mut()
            .ok_or(WalletError::WalletLocked)?
            .add_hardware(record)?;
        self.persist_hardware()?;
        Ok(account)
    }

    /// Preview Ledger Live paths `0..4` (device must be unlocked + Ethereum app).
    pub async fn preview_ledger_accounts(&self) -> Result<Vec<(String, String)>, WalletError> {
        let chain_id = self.networks.active().chain_id;
        crate::security::preview_ledger_live_paths(5, Some(chain_id)).await
    }

    /// Discover a Ledger account at `path` and add it as a watch record.
    pub async fn add_ledger_account(
        &mut self,
        path: &str,
        label: &str,
    ) -> Result<crate::core::account::Account, WalletError> {
        self.require_unlocked()?;
        let net = self.networks.active();
        let record = crate::security::discover_ledger_account(
            path,
            Some(net.chain_id),
            Some(net.chain_id.to_string()),
            label,
        )
        .await?;
        self.add_hardware_account(record)
    }

    /// CI/Anvil: sign hardware accounts with `mock` instead of USB (address must match).
    pub fn set_hardware_mock(&mut self, mock: crate::security::MockSignerBackend) {
        self.hw_mock = Some(mock);
    }

    fn persist_hardware(&mut self) -> Result<(), WalletError> {
        let hardware = self
            .accounts
            .as_ref()
            .ok_or(WalletError::WalletLocked)?
            .hardware()
            .to_vec();
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        persisted.hardware = hardware;
        if let Some(accounts) = self.accounts.as_ref() {
            persisted.active_account_index = accounts.active_index();
        }
        self.state.save(persisted)?;
        Ok(())
    }

    /// Display label for account `index` (F3 chrome preview).
    pub fn account_label(&self, index: u32) -> Result<String, WalletError> {
        self.require_unlocked()?
            .label_for(index)
            .map(str::to_string)
            .ok_or_else(|| WalletError::AccountNotFound(format!("account index {index}")))
    }

    /// All accounts as `(index, label)` for F3 cycling.
    pub fn account_choices(&self) -> Result<Vec<(u32, String)>, WalletError> {
        Ok(self
            .require_unlocked()?
            .accounts()
            .iter()
            .map(|a| (a.index, a.label.clone()))
            .collect())
    }

    /// The active account's signing key (requires an unlocked **software** wallet).
    ///
    /// Exposed for flows that sign outside the built-in send path (e.g. the
    /// AA batched-send view); the caller drops the key when done. Hardware
    /// accounts return [`WalletError::HardwareUnsupported`].
    pub fn active_signer(&self) -> Result<PrivateKeySigner, WalletError> {
        self.require_unlocked()?.active_signer()
    }

    /// Local [`LocalSignerBackend`] for the active software account.
    pub fn active_local_backend(&self) -> Result<crate::security::LocalSignerBackend, WalletError> {
        self.require_unlocked()?.active_local_backend()
    }

    /// The effective RPC URL of the active network (override if set).
    pub fn active_rpc_url(&self) -> String {
        self.effective_rpc()
    }

    /// Snapshot active network RPC endpoints for read-only provider proxying.
    ///
    /// Does not require an unlocked wallet — only initialized vault + network config.
    pub fn network_rpc_snapshot(&self) -> Result<NetworkRpcSnapshot, WalletError> {
        if !self.is_initialized() {
            return Err(WalletError::NotInitialized);
        }
        let net = self.networks().active();
        let (rpc_url, fallback_rpc_urls) = self.rpc_endpoints_for(net);
        Ok(NetworkRpcSnapshot {
            rpc_url,
            fallback_rpc_urls,
            chain_id: net.chain_id,
            network_name: net.name.clone(),
        })
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
        let mut secrets = VaultSecrets::from_mnemonic_phrase(mnemonic.to_string());
        let mut encoded = secrets.encode()?;
        let vault = encrypt(encoded.as_bytes(), password)?;
        encoded.zeroize();
        secrets.zeroize();

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
        let accounts = self.unlock_payload()?.decrypt(password)?;
        self.apply_unlocked_accounts(accounts);
        Ok(())
    }

    /// Clone everything the expensive half of [`Self::unlock`] needs, so the
    /// Argon2id KDF can run off the UI thread without holding the wallet lock
    /// (a multi-second KDF under the mutex would freeze the TUI render loop).
    pub fn unlock_payload(&self) -> Result<UnlockPayload, WalletError> {
        let persisted = self.persisted.as_ref().ok_or(WalletError::NotInitialized)?;
        Ok(UnlockPayload {
            vault: persisted.vault.clone(),
            hardware: persisted.hardware.clone(),
            active_account_index: persisted.active_account_index,
        })
    }

    /// Install accounts produced by [`UnlockPayload::decrypt`] (off-thread KDF).
    pub fn apply_unlocked_accounts(&mut self, accounts: AccountManager) {
        self.accounts = Some(accounts);
    }

    /// Re-encrypt the current unlocked secrets under `password` (must match vault).
    fn persist_unlocked_secrets(&mut self, password: &SecretString) -> Result<(), WalletError> {
        let accounts = self.accounts.as_ref().ok_or(WalletError::WalletLocked)?;
        let mut secrets = accounts.to_secrets();
        let mut encoded = secrets.encode()?;
        let vault = encrypt(encoded.as_bytes(), password)?;
        encoded.zeroize();
        secrets.zeroize();
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        persisted.vault = vault;
        self.state.save(persisted)?;
        Ok(())
    }

    /// Confirm `password` against the vault (wrong password → [`WalletError::DecryptionFailed`]).
    pub fn verify_password(&self, password: &SecretString) -> Result<(), WalletError> {
        let persisted = self.persisted.as_ref().ok_or(WalletError::NotInitialized)?;
        let mut plaintext = decrypt(&persisted.vault, password)?;
        plaintext.zeroize();
        Ok(())
    }

    /// Export the BIP-39 recovery phrase after password confirmation.
    ///
    /// Refuses when the active account is hardware (Keys UX: no seed reveal
    /// while F3 is on a device watch account).
    pub fn export_mnemonic(&self, password: &SecretString) -> Result<SecretString, WalletError> {
        self.require_unlocked()?.require_software_active()?;
        self.verify_password(password)?;
        Ok(self.require_unlocked()?.mnemonic_phrase())
    }

    /// Export the active (F3) account's private key (hex) after password confirmation.
    ///
    /// The returned key is verified to derive the active account's address before
    /// it leaves this method — so Keys cannot show a mismatched secret.
    pub fn export_active_private_key(
        &self,
        password: &SecretString,
    ) -> Result<SecretString, WalletError> {
        let accounts = self.require_unlocked()?;
        self.verify_password(password)?;
        let active = accounts.active_account();
        let index = active.index;
        let expected = active.address.clone();
        let sk = accounts.export_private_key(index)?;
        let signer = crate::core::vault_secrets::parse_private_key(sk.expose_secret())?;
        if !format!("{}", signer.address()).eq_ignore_ascii_case(&expected) {
            return Err(WalletError::Other(
                "exported key does not match the F3 active account — aborting reveal".into(),
            ));
        }
        Ok(sk)
    }

    /// Import a hex private key into the vault (password-gated rewrite).
    pub fn import_private_key(
        &mut self,
        password: &SecretString,
        label: &str,
        private_key: &SecretString,
    ) -> Result<crate::core::account::Account, WalletError> {
        self.require_unlocked()?;
        self.verify_password(password)?;
        let account = self
            .accounts
            .as_mut()
            .ok_or(WalletError::WalletLocked)?
            .import_private_key(label, private_key)?;
        self.persist_unlocked_secrets(password)?;
        if let Some(persisted) = self.persisted.as_mut() {
            persisted.active_account_index = account.index;
            self.state.save(persisted)?;
        }
        Ok(account)
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

    /// User-defined networks stored in the vault metadata.
    pub fn custom_networks(&self) -> &[crate::core::persistence::CustomNetwork] {
        self.persisted
            .as_ref()
            .map(|p| p.custom_networks.as_slice())
            .unwrap_or(&[])
    }

    /// Add a custom EVM network and select it.
    pub fn add_custom_network(
        &mut self,
        name: &str,
        chain_id: u64,
        rpc_url: &str,
        native_symbol: &str,
        is_testnet: bool,
    ) -> Result<crate::core::persistence::CustomNetwork, WalletError> {
        let name = name.trim();
        let rpc_url = rpc_url.trim();
        let symbol = {
            let s = native_symbol.trim();
            if s.is_empty() {
                "ETH"
            } else {
                s
            }
        };
        if name.is_empty() {
            return Err(WalletError::InvalidTransaction(
                "network name is required".into(),
            ));
        }
        let rpc_url = Self::normalize_rpc_url(rpc_url)?;
        if chain_id == 0 {
            return Err(WalletError::InvalidTransaction(
                "chain id must be non-zero".into(),
            ));
        }
        // Collision with built-in or existing custom.
        if self
            .networks
            .networks()
            .iter()
            .any(|n| n.chain_id == chain_id)
        {
            return Err(WalletError::Other(format!(
                "a network with chain id {chain_id} already exists"
            )));
        }
        let id = format!("custom-{chain_id}");
        if self.networks.get(&id).is_some() {
            return Err(WalletError::Other(format!(
                "custom network id `{id}` already exists"
            )));
        }
        let custom = crate::core::persistence::CustomNetwork {
            id: id.clone(),
            name: name.to_string(),
            chain_id,
            rpc_url: rpc_url.clone(),
            native_symbol: symbol.to_string(),
            is_testnet,
        };
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        persisted.custom_networks.push(custom.clone());
        persisted.active_network_id = id;
        self.state.save(persisted)?;
        self.networks
            .reload_custom(&persisted.custom_networks.clone())?;
        Ok(custom)
    }

    /// Update a custom network's metadata (chain id is fixed at add time).
    pub fn update_custom_network(
        &mut self,
        id: &str,
        name: &str,
        rpc_url: &str,
        native_symbol: &str,
        is_testnet: bool,
    ) -> Result<crate::core::persistence::CustomNetwork, WalletError> {
        if !self.networks.is_custom(id) {
            return Err(WalletError::Other(
                "built-in networks cannot be edited — use Settings r for RPC".into(),
            ));
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(WalletError::InvalidTransaction(
                "network name is required".into(),
            ));
        }
        let rpc_url = Self::normalize_rpc_url(rpc_url)?;
        let symbol = {
            let s = native_symbol.trim();
            if s.is_empty() {
                "ETH"
            } else {
                s
            }
        };
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        let key = id.trim().to_ascii_lowercase();
        let custom = persisted
            .custom_networks
            .iter_mut()
            .find(|n| n.id.eq_ignore_ascii_case(&key))
            .ok_or_else(|| WalletError::NetworkNotFound(id.to_string()))?;
        custom.name = name.to_string();
        custom.rpc_url = rpc_url;
        custom.native_symbol = symbol.to_string();
        custom.is_testnet = is_testnet;
        persisted.network_rpc_primary.remove(&key);
        let updated = custom.clone();
        let customs = persisted.custom_networks.clone();
        self.state.save(persisted)?;
        self.networks.reload_custom(&customs)?;
        Ok(updated)
    }

    /// Remove a custom network by id. Built-ins cannot be removed.
    pub fn remove_custom_network(&mut self, id: &str) -> Result<(), WalletError> {
        if !self.networks.is_custom(id) {
            return Err(WalletError::Other(
                "built-in networks cannot be removed".into(),
            ));
        }
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        let before = persisted.custom_networks.len();
        persisted
            .custom_networks
            .retain(|n| !n.id.eq_ignore_ascii_case(id.trim()));
        if persisted.custom_networks.len() == before {
            return Err(WalletError::NetworkNotFound(id.to_string()));
        }
        persisted
            .network_rpc_primary
            .remove(&id.trim().to_ascii_lowercase());
        let was_active = persisted.active_network_id.eq_ignore_ascii_case(id.trim());
        if was_active {
            persisted.active_network_id = crate::core::network::DEFAULT_NETWORK_ID.to_string();
        }
        let customs = persisted.custom_networks.clone();
        let active = persisted.active_network_id.clone();
        self.state.save(persisted)?;
        self.networks = NetworkService::with_custom(active, &customs)?;
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
        self.adapter_for(self.networks.active()).await
    }

    /// Native balance of the active account on the active network.
    pub async fn balance(&self) -> Result<Balance, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = self.adapter_for(net).await?;
        adapter.get_balance(address).await
    }

    /// Snapshot RPC + account facts so the TUI can drop the wallet mutex before network I/O.
    ///
    /// Holding [`WalletState`] across `eth_getBalance` / gas RPCs freezes the UI on
    /// `[busy]` — chrome refresh must use this snapshot pattern instead.
    pub fn chrome_rpc_snapshot(&self) -> Result<ChromeRpcSnapshot, WalletError> {
        let (net, address) = self.active_context()?;
        let (rpc_url, fallback_rpc_urls) = self.rpc_endpoints_for(net);
        Ok(ChromeRpcSnapshot {
            rpc_url,
            fallback_rpc_urls,
            chain_id: net.chain_id,
            network_name: net.name.clone(),
            address: address.to_string(),
        })
    }

    /// Suggested max fee / legacy gas price as a gwei display string for wallet chrome.
    ///
    /// Prefers Alloy EIP-1559 feeHistory estimates; falls back to `eth_gasPrice`.
    pub async fn gas_price_gwei_display(&self) -> Result<String, WalletError> {
        let snap = self.chrome_rpc_snapshot()?;
        snap.gas_price_gwei_display().await
    }

    /// All detected balances of the active account: the native asset plus
    /// curated / discovered / user-imported ERC-20s.
    ///
    /// ERC-20s are read in one Multicall3 `tryAggregate` batch (EIP-20 +
    /// mds1/multicall; see `docs/optimizations.md` for provenance), with a
    /// sequential fallback when Multicall3 is absent. Zero balances are
    /// excluded except for user-imported custom tokens; symbol/decimals come
    /// from the contract (cached), falling back to the curated registry.
    pub async fn assets(&self) -> Result<Vec<Balance>, WalletError> {
        let (net, address) = self.active_context()?;
        let extras: Vec<String> = self
            .custom_tokens_for_active_chain()
            .into_iter()
            .map(|t| t.address)
            .collect();
        let adapter = self.adapter_for(net).await?;
        adapter.get_assets(address, &extras).await
    }

    /// Recent ERC-20 Transfer activity for the active account (newest first).
    pub async fn activity(&self, limit: u32) -> Result<Vec<crate::chains::TxRecord>, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = self.adapter_for(net).await?;
        adapter.get_transaction_history(address, limit).await
    }

    /// Non-zero allowances of held ERC-20s against known Ag / Dex / Bridge spenders.
    pub async fn list_allowances(&self) -> Result<Vec<crate::chains::AllowanceEntry>, WalletError> {
        use crate::core::aggregator::OFFICIAL_AGG_ROUTERS;
        use crate::core::bridge::OFFICIAL_ROUTERS;
        use crate::core::dex_routers_labeled;
        use alloy::primitives::{Address, U256};
        use std::str::FromStr;

        let (net, address) = self.active_context()?;
        let owner = Address::from_str(address)
            .map_err(|_| WalletError::InvalidTransaction(format!("active address: {address}")))?;
        let assets = self.assets().await?;
        let adapter = self.adapter_for(net).await?;

        let mut spenders: Vec<(Address, &'static str)> = dex_routers_labeled(net.chain_id);
        for s in OFFICIAL_AGG_ROUTERS {
            if let Ok(a) = Address::from_str(s) {
                spenders.push((a, "Ag"));
            }
        }
        if net.chain_id == 369 || net.chain_id == 943 {
            for s in OFFICIAL_ROUTERS {
                if let Ok(a) = Address::from_str(s) {
                    spenders.push((a, "Bridge"));
                }
            }
        }
        // Dedup spenders
        spenders.sort_by_key(|(a, _)| *a);
        spenders.dedup_by_key(|(a, _)| *a);

        let mut out = Vec::new();
        for bal in &assets {
            let Some(ref ca) = bal.token.contract_address else {
                continue;
            };
            let Ok(token) = Address::from_str(ca) else {
                continue;
            };
            for (spender, label) in &spenders {
                match adapter.get_erc20_allowance(token, owner, *spender).await {
                    Ok(amount) if amount > U256::ZERO => {
                        out.push(crate::chains::AllowanceEntry {
                            token: format!("{token:#x}"),
                            token_symbol: bal.token.symbol.clone(),
                            token_decimals: bal.token.decimals,
                            spender: format!("{spender:#x}"),
                            spender_label: (*label).to_string(),
                            amount: amount.to_string(),
                        });
                    }
                    _ => {}
                }
            }
        }
        Ok(out)
    }

    /// Balance of a single ERC-20 (`token_address`) for the active account.
    pub async fn token_balance(&self, token_address: &str) -> Result<Balance, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = self.adapter_for(net).await?;
        adapter.get_token_balance(token_address, address).await
    }

    /// Import an ERC-20 by contract address (reads on-chain metadata, persists).
    pub async fn import_custom_token(
        &mut self,
        token_address: &str,
    ) -> Result<CustomToken, WalletError> {
        let (net, _) = self.active_context()?;
        let adapter = self.adapter_for(net).await?;
        // Validate address + that the contract responds to balanceOf/metadata.
        let _ = adapter
            .get_token_balance(token_address, self.active_address()?)
            .await?;
        let (symbol, name, decimals) = adapter.get_token_metadata(token_address).await?;
        let token = CustomToken {
            chain_id: net.chain_id,
            address: {
                // Checksum via alloy parse round-trip.
                use alloy::primitives::Address;
                use std::str::FromStr;
                Address::from_str(token_address.trim())
                    .map(|a| format!("{a:#x}"))
                    .map_err(|_| WalletError::InvalidTransaction("invalid token address".into()))?
            },
            symbol,
            name,
            decimals,
        };
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        if persisted
            .custom_tokens
            .iter()
            .any(|t| t.chain_id == token.chain_id && t.address.eq_ignore_ascii_case(&token.address))
        {
            return Ok(token);
        }
        persisted.custom_tokens.push(token.clone());
        self.state.save(persisted)?;
        Ok(token)
    }

    /// Custom tokens stored for the active chain.
    pub fn custom_tokens_for_active_chain(&self) -> Vec<CustomToken> {
        let chain_id = self.networks().active().chain_id;
        self.persisted
            .as_ref()
            .map(|p| {
                p.custom_tokens
                    .iter()
                    .filter(|t| t.chain_id == chain_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Remove a custom token by address on the active chain.
    pub fn remove_custom_token(&mut self, token_address: &str) -> Result<(), WalletError> {
        let chain_id = self.networks().active().chain_id;
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        let before = persisted.custom_tokens.len();
        persisted.custom_tokens.retain(|t| {
            !(t.chain_id == chain_id && t.address.eq_ignore_ascii_case(token_address.trim()))
        });
        if persisted.custom_tokens.len() == before {
            return Err(WalletError::Other(
                "token is not in your custom list".into(),
            ));
        }
        self.state.save(persisted)?;
        Ok(())
    }

    /// Whitelisted dApps (launcher + provider origins).
    pub fn trusted_dapps(&self) -> Vec<TrustedDapp> {
        self.persisted
            .as_ref()
            .map(|p| p.trusted_dapps.clone())
            .unwrap_or_default()
    }

    /// Add a dApp to the whitelist (name + https URL).
    pub fn add_trusted_dapp(&mut self, name: &str, url: &str) -> Result<TrustedDapp, WalletError> {
        let url = url.trim();
        let parsed = url::Url::parse(url).map_err(|_| {
            WalletError::InvalidTransaction("dApp URL must be a valid http(s) URL".into())
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(WalletError::InvalidTransaction(
                "dApp URL must be http or https".into(),
            ));
        }
        let dapp = TrustedDapp {
            name: if name.trim().is_empty() {
                parsed.host_str().unwrap_or("dApp").to_string()
            } else {
                name.trim().to_string()
            },
            url: url.to_string(),
            extra_hosts: Vec::new(),
        };
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        if persisted
            .trusted_dapps
            .iter()
            .any(|d| d.url.eq_ignore_ascii_case(&dapp.url))
        {
            return Ok(dapp);
        }
        persisted.trusted_dapps.push(dapp.clone());
        self.state.save(persisted)?;
        Ok(dapp)
    }

    /// Remove a whitelisted dApp by URL.
    pub fn remove_trusted_dapp(&mut self, url: &str) -> Result<(), WalletError> {
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        let before = persisted.trusted_dapps.len();
        persisted
            .trusted_dapps
            .retain(|d| !d.url.eq_ignore_ascii_case(url.trim()));
        if persisted.trusted_dapps.len() == before {
            return Err(WalletError::Other("dApp is not in the whitelist".into()));
        }
        self.state.save(persisted)?;
        Ok(())
    }

    /// Whether loopback CDP agent control is enabled for VB (FR-7.5).
    pub fn agent_browser_control(&self) -> bool {
        self.persisted
            .as_ref()
            .is_some_and(|p| p.agent_browser_control)
    }

    /// Enable or disable agent browser control (CDP); persists immediately.
    pub fn set_agent_browser_control(&mut self, enabled: bool) -> Result<(), WalletError> {
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        persisted.agent_browser_control = enabled;
        self.state.save(persisted)?;
        if !enabled {
            crate::core::vb_browser::clear_vb_session();
        }
        Ok(())
    }

    /// MCP/VB connect autonomy tier for this profile.
    pub fn agent_autonomy_tier(&self) -> crate::core::AgentAutonomyTier {
        self.persisted
            .as_ref()
            .map(|p| p.agent_autonomy_tier)
            .unwrap_or_default()
    }

    /// Set agent autonomy tier (advisor vs operator auto-connect).
    pub fn set_agent_autonomy_tier(
        &mut self,
        tier: crate::core::AgentAutonomyTier,
    ) -> Result<(), WalletError> {
        let persisted = self.persisted.as_mut().ok_or(WalletError::NotInitialized)?;
        persisted.agent_autonomy_tier = tier;
        self.state.save(persisted)?;
        Ok(())
    }

    /// Origins derived from trusted dApps (for the provider allowlist).
    pub fn trusted_dapp_origins(&self) -> Vec<String> {
        self.trusted_dapps()
            .iter()
            .filter_map(|d| {
                let u = url::Url::parse(&d.url).ok()?;
                let origin = u.origin().ascii_serialization();
                if origin == "null" {
                    None
                } else {
                    Some(origin)
                }
            })
            .collect()
    }

    /// Estimate fee for an ERC-20 `transfer`.
    pub async fn estimate_token_fee(
        &self,
        token: &str,
        to: &str,
        amount: &str,
    ) -> Result<Fee, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = self.adapter_for(net).await?;
        let service = TransactionService::new();
        let tx = service.build_erc20_transfer(address, token, to, amount, net.chain_id)?;
        service.estimate_fee(&adapter, &tx).await
    }

    /// Broadcast an ERC-20 transfer (caller must have shown fee + gotten approval).
    pub async fn send_token(
        &self,
        token: &str,
        to: &str,
        amount: &str,
    ) -> Result<crate::core::broadcasts::BroadcastReceipt, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = self.adapter_for(net).await?;
        let service = TransactionService::new();
        let mut tx = service.build_erc20_transfer(address, token, to, amount, net.chain_id)?;
        let fee = service.estimate_fee(&adapter, &tx).await?;
        service.apply_fee(&mut tx, &fee)?;
        let ChainTransaction::Evm(evm_tx) = tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".into(),
            ));
        };
        self.broadcast(evm_tx, "Token").await
    }

    /// ERC-20 transfer using an already-approved fee.
    pub async fn send_token_with_fee(
        &self,
        token: &str,
        to: &str,
        amount: &str,
        fee: &Fee,
    ) -> Result<crate::core::broadcasts::BroadcastReceipt, WalletError> {
        let (net, address) = self.active_context()?;
        let service = TransactionService::new();
        let mut tx = service.build_erc20_transfer(address, token, to, amount, net.chain_id)?;
        service.apply_fee(&mut tx, fee)?;
        let ChainTransaction::Evm(evm_tx) = tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".into(),
            ));
        };
        self.broadcast(evm_tx, "Token").await
    }

    /// Estimate the fee to send `value_wei` (base units) to `to`.
    pub async fn estimate_fee(&self, to: &str, value_wei: &str) -> Result<Fee, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = self.adapter_for(net).await?;
        let service = TransactionService::new();
        let tx = service.build_native_transfer(address, to, value_wei, net.chain_id)?;
        service.estimate_fee(&adapter, &tx).await
    }

    /// Build, estimate, sign, and broadcast a native transfer. The caller (UI)
    /// must have shown the user the fee and obtained explicit approval first.
    pub async fn send(
        &self,
        to: &str,
        value_wei: &str,
    ) -> Result<crate::core::broadcasts::BroadcastReceipt, WalletError> {
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
        self.broadcast(evm_tx, "Send").await
    }

    /// Native transfer using an already-approved fee (e.g. Slow/Normal/Fast/Ape).
    ///
    /// Does not re-estimate; `fee` is applied verbatim so the UI confirmation
    /// matches what is signed.
    pub async fn send_with_fee(
        &self,
        to: &str,
        value_wei: &str,
        fee: &Fee,
    ) -> Result<crate::core::broadcasts::BroadcastReceipt, WalletError> {
        let accounts = self.require_unlocked()?;
        let net = self.networks.active();
        let service = TransactionService::new();
        let mut tx = service.build_native_transfer(
            accounts.active_address(),
            to,
            value_wei,
            net.chain_id,
        )?;
        service.apply_fee(&mut tx, fee)?;
        let ChainTransaction::Evm(evm_tx) = tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".to_string(),
            ));
        };
        self.broadcast(evm_tx, "Send").await
    }

    /// Build, estimate, sign, and broadcast an arbitrary EVM transaction
    /// (native transfer or contract call, with optional `data`). Missing
    /// gas/fee parameters are filled from a fee estimate. The caller must have
    /// shown the user the request and obtained explicit approval first.
    pub async fn send_transaction(&self, tx: EvmTransaction) -> Result<TxHash, WalletError> {
        let receipt = self.broadcast(tx, "tx").await?;
        Ok(TxHash(receipt.hash))
    }

    /// Like [`Self::send_transaction`], but returns a [`BroadcastReceipt`] for
    /// History cancel / speed-up (nonce + fees captured).
    pub async fn broadcast(
        &self,
        tx: EvmTransaction,
        label: &str,
    ) -> Result<crate::core::broadcasts::BroadcastReceipt, WalletError> {
        use crate::core::broadcasts::{BroadcastEntry, BroadcastReceipt};

        let (adapter, prepared, raw) = self.prepare_sign_raw(tx).await?;
        let hash = adapter.broadcast_raw(raw).await?;
        adapter.invalidate_balance_cache().await;
        let entry = BroadcastEntry::from_prepared(&prepared, hash.0.clone(), label);
        Ok(BroadcastReceipt {
            hash: hash.0,
            entry,
        })
    }

    /// Broadcast a fixed-supply token deploy and return RPC context for receipt polling.
    async fn token_launch_broadcast(
        &mut self,
        name: &str,
        symbol: &str,
        supply_human: &str,
    ) -> Result<
        (
            crate::core::broadcasts::BroadcastReceipt,
            String,
            Vec<String>,
            u64,
            String,
        ),
        WalletError,
    > {
        use crate::core::token_launch::build_erc20_deploy_evm;
        use alloy::primitives::Address;
        use std::str::FromStr;

        let accounts = self.require_unlocked()?;
        let net = self.networks.active();
        let from = accounts.active_address();
        let recipient = Address::from_str(from)
            .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?;
        let tx = build_erc20_deploy_evm(from, net.chain_id, name, symbol, supply_human, recipient)?;
        let receipt = self.broadcast(tx, "Token launch").await?;
        let (rpc_url, fallback_rpc_urls) = self.rpc_endpoints_for(net);
        Ok((
            receipt,
            rpc_url,
            fallback_rpc_urls,
            net.chain_id,
            net.name.clone(),
        ))
    }

    /// Deploy a fixed-supply ERC-20 (testnet meme launch), wait for the contract
    /// address, and import it into the profile asset list.
    pub async fn deploy_fixed_supply_token(
        &mut self,
        name: &str,
        symbol: &str,
        supply_human: &str,
    ) -> Result<crate::core::token_launch::TokenLaunchOutcome, WalletError> {
        use crate::core::token_launch::{wait_for_deployed_address, TokenLaunchOutcome};
        use std::time::Duration;

        let (receipt, rpc_url, fallback_rpc_urls, chain_id, network_name) = self
            .token_launch_broadcast(name, symbol, supply_human)
            .await?;
        let adapter =
            EvmAdapter::new(&rpc_url, chain_id, &network_name, &fallback_rpc_urls).await?;
        let contract =
            wait_for_deployed_address(&adapter, &receipt.hash, Duration::from_secs(90)).await?;
        let token = self.import_custom_token(&format!("{contract:#x}")).await?;
        Ok(TokenLaunchOutcome {
            tx_hash: receipt.hash,
            contract,
            token,
        })
    }

    /// Like [`Self::deploy_fixed_supply_token`] but releases the wallet mutex while
    /// waiting for the deploy receipt (background TUI jobs must not freeze the UI).
    #[allow(clippy::await_holding_lock)] // lock is scoped to sign/broadcast and import only; receipt poll is unlocked
    pub async fn deploy_fixed_supply_token_background(
        wallet: &std::sync::Mutex<Self>,
        name: &str,
        symbol: &str,
        supply_human: &str,
    ) -> Result<crate::core::token_launch::TokenLaunchOutcome, WalletError> {
        use crate::core::token_launch::{wait_for_deployed_address, TokenLaunchOutcome};
        use std::time::Duration;

        let (receipt, rpc_url, fallback_rpc_urls, chain_id, network_name) = {
            let mut w = wallet
                .lock()
                .map_err(|_| WalletError::InvalidTransaction("wallet lock unavailable".into()))?;
            w.token_launch_broadcast(name, symbol, supply_human).await?
        };
        let adapter =
            EvmAdapter::new(&rpc_url, chain_id, &network_name, &fallback_rpc_urls).await?;
        let contract =
            wait_for_deployed_address(&adapter, &receipt.hash, Duration::from_secs(90)).await?;
        let token = {
            let mut w = wallet
                .lock()
                .map_err(|_| WalletError::InvalidTransaction("wallet lock unavailable".into()))?;
            w.import_custom_token(&format!("{contract:#x}")).await?
        };
        Ok(TokenLaunchOutcome {
            tx_hash: receipt.hash,
            contract,
            token,
        })
    }

    /// Cancel or speed-up a pending [`BroadcastEntry`] (same nonce, bumped fees).
    pub async fn replace_broadcast(
        &self,
        entry: &crate::core::broadcasts::BroadcastEntry,
        kind: crate::core::broadcasts::ReplaceKind,
    ) -> Result<crate::core::broadcasts::BroadcastReceipt, WalletError> {
        if !entry.is_replaceable() {
            return Err(WalletError::InvalidTransaction(
                "transaction is not pending — cannot replace".into(),
            ));
        }
        let net = self.networks.active();
        if entry.chain_id != net.chain_id {
            return Err(WalletError::InvalidTransaction(format!(
                "broadcast is on chain {} but wallet is on {}",
                entry.chain_id, net.chain_id
            )));
        }
        let tx = entry.replacement_tx(kind)?;
        let mut receipt = self.broadcast(tx, kind.label()).await?;
        receipt.entry.replaces = Some(entry.hash.clone());
        Ok(receipt)
    }

    /// Poll inclusion status for a previously broadcast hash (Pending /
    /// Confirmed / Failed). Used by the Send Done screen.
    pub async fn get_tx_status(&self, tx_hash: &str) -> Result<TxStatus, WalletError> {
        let adapter = self.active_adapter().await?;
        adapter.get_tx_status(tx_hash).await
    }

    /// Contract call (or native transfer) using an already-approved fee.
    ///
    /// Does not re-estimate; `fee` is applied verbatim so the confirmation
    /// matches what is signed (DEX / bridge / Ag flows).
    pub async fn send_evm_with_fee(
        &self,
        tx: EvmTransaction,
        fee: &Fee,
    ) -> Result<crate::core::broadcasts::BroadcastReceipt, WalletError> {
        let service = TransactionService::new();
        let mut chain_tx = ChainTransaction::Evm(tx);
        service.apply_fee(&mut chain_tx, fee)?;
        let ChainTransaction::Evm(evm_tx) = chain_tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".to_string(),
            ));
        };
        self.broadcast(evm_tx, "Contract").await
    }

    /// Sign an EVM transaction without broadcasting it; returns the raw signed
    /// tx as `0x`-prefixed hex (serves `vaughan_signTransaction`).
    pub async fn sign_transaction(&self, tx: EvmTransaction) -> Result<String, WalletError> {
        let (_adapter, _prepared, raw) = self.prepare_sign_raw(tx).await?;
        Ok(format!("0x{}", hex::encode(raw)))
    }

    /// Estimate the fee for an arbitrary EVM transaction payload.
    ///
    /// Used by provider approval UX to show the user a fee before signing.
    pub async fn estimate_transaction_fee(&self, tx: EvmTransaction) -> Result<Fee, WalletError> {
        self.require_unlocked()?;
        let net = self.networks.active();
        let adapter = self.adapter_for(net).await?;
        adapter.estimate_fee(&ChainTransaction::Evm(tx)).await
    }

    /// Sign `message` as an EIP-191 personal message with the active account;
    /// returns the signature as a `0x`-prefixed hex string.
    ///
    /// Hardware accounts require a Tokio runtime (confirm on device).
    pub fn sign_message(&self, message: &[u8]) -> Result<String, WalletError> {
        if self.active_is_hardware().unwrap_or(false) {
            return block_on_wallet(self.sign_message_async(message));
        }
        let backend = self.active_local_backend()?;
        crate::security::signing::sign_personal_message(backend.local_signer(), message)
    }

    /// Async personal-sign (Ledger confirm-on-device when active is hardware).
    pub async fn sign_message_async(&self, message: &[u8]) -> Result<String, WalletError> {
        use crate::security::{SignRequest, SignResult};
        let backend = self.owned_active_backend()?;
        match backend
            .sign(SignRequest::EvmPersonal {
                message: message.to_vec(),
            })
            .await?
        {
            SignResult::SignatureHex(s) => Ok(s),
            SignResult::RawTx(_) => {
                Err(WalletError::SigningFailed("expected signature hex".into()))
            }
        }
    }

    /// Sign an EIP-712 typed-data payload with the active account; returns the
    /// signature as a `0x`-prefixed hex string.
    pub fn sign_typed_data(&self, typed_data: &serde_json::Value) -> Result<String, WalletError> {
        if self.active_is_hardware().unwrap_or(false) {
            return block_on_wallet(self.sign_typed_data_async(typed_data.clone()));
        }
        let backend = self.active_local_backend()?;
        backend.sign_typed_data_json(typed_data)
    }

    /// Async EIP-712 sign (full JSON — required for Ledger).
    pub async fn sign_typed_data_async(
        &self,
        typed_data: serde_json::Value,
    ) -> Result<String, WalletError> {
        use crate::security::{SignRequest, SignResult};
        let backend = self.owned_active_backend()?;
        match backend
            .sign(SignRequest::EvmTypedData {
                payload: typed_data,
            })
            .await?
        {
            SignResult::SignatureHex(s) => Ok(s),
            SignResult::RawTx(_) => {
                Err(WalletError::SigningFailed("expected signature hex".into()))
            }
        }
    }

    /// Prepare fees/nonce on an unsigned adapter, sign via active backend,
    /// return `(adapter, prepared_tx, raw_envelope)` for broadcast or return.
    async fn prepare_sign_raw(
        &self,
        mut tx: EvmTransaction,
    ) -> Result<(EvmAdapter, EvmTransaction, Vec<u8>), WalletError> {
        use crate::security::{SignRequest, SignResult};

        let accounts = self.require_unlocked()?;
        let net = self.networks.active();
        // Ensure `from` matches active account when left default by callers.
        if tx.from.is_empty() {
            tx.from = accounts.active_address().to_string();
        }
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
            tx = prepared;
        }
        if tx.nonce.is_none() {
            tx.nonce = Some(adapter.get_pending_nonce(&tx.from).await?);
        }
        let backend = self.owned_active_backend()?;
        let raw = match backend
            .sign(SignRequest::EvmTransaction { tx: tx.clone() })
            .await?
        {
            SignResult::RawTx(raw) => raw,
            SignResult::SignatureHex(_) => {
                return Err(WalletError::SigningFailed(
                    "expected raw transaction envelope".into(),
                ));
            }
        };
        Ok((adapter, tx, raw))
    }

    fn owned_active_backend(&self) -> Result<OwnedSignerBackend, WalletError> {
        use crate::security::{AccountKind, HardwareVendor, LedgerSignerBackend};

        let accounts = self.require_unlocked()?;
        let chain_id = self.networks.active().chain_id;
        match &accounts.active_account().kind {
            AccountKind::Hardware(rec) => {
                if let Some(mock) = &self.hw_mock {
                    if !mock.address_string().eq_ignore_ascii_case(&rec.address) {
                        return Err(WalletError::HardwareUnsupported(
                            "hardware mock address does not match active watch account".into(),
                        ));
                    }
                    return Ok(OwnedSignerBackend::Mock(mock.clone()));
                }
                match rec.vendor {
                    HardwareVendor::Ledger => Ok(OwnedSignerBackend::Ledger(
                        LedgerSignerBackend::new(rec.clone(), Some(chain_id))?,
                    )),
                    HardwareVendor::Trezor => Err(WalletError::HardwareUnsupported(
                        "Trezor support is Phase 2 — not enabled yet".into(),
                    )),
                }
            }
            AccountKind::Hd | AccountKind::Imported => {
                Ok(OwnedSignerBackend::Local(accounts.active_local_backend()?))
            }
        }
    }

    /// Build a signer-backed adapter for the active network and prepare `tx`
    /// (fill missing gas/fees) for signing or broadcast.
    ///
    /// Prefer [`Self::prepare_sign_raw`] for new paths; kept for callers that
    /// still use [`EvmAdapter`] with an in-process signer.
    #[allow(dead_code)]
    async fn signed_adapter_and_tx(
        &self,
        mut tx: EvmTransaction,
    ) -> Result<(EvmAdapter, EvmTransaction), WalletError> {
        let accounts = self.require_unlocked()?;
        accounts.require_software_active()?;
        let net = self.networks.active();
        let signer = accounts.active_signer()?;
        let (primary, fallbacks) = self.rpc_endpoints_for(net);
        let adapter =
            EvmAdapter::with_signer(&primary, net.chain_id, &net.name, signer, &fallbacks).await?;
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

    /// Unlocked accounts for sibling core modules (stealth send/scan/sweep).
    pub(crate) fn unlocked_accounts(&self) -> Result<&AccountManager, WalletError> {
        self.require_unlocked()
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
    fn export_active_private_key_matches_f3_account() {
        let mut w = WalletState::load(tmp_path()).unwrap();
        w.create(&password(), mnemonic()).unwrap();
        w.set_active_account(1).unwrap();
        let (label, address, imported) = w.active_account_export_context().unwrap();
        assert_eq!(label, "wallet 1");
        assert!(!imported);

        let sk = w.export_active_private_key(&password()).unwrap();
        let signer = crate::core::vault_secrets::parse_private_key(sk.expose_secret()).unwrap();
        assert_eq!(
            format!("{}", signer.address()).to_lowercase(),
            address.to_lowercase()
        );

        w.set_active_account(0).unwrap();
        let sk0 = w.export_active_private_key(&password()).unwrap();
        assert_ne!(sk.expose_secret(), sk0.expose_secret());
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

    #[test]
    fn stealth_uri_uses_eip3770_prefix_when_unlocked() {
        let mut w = WalletState::load(tmp_path()).unwrap();
        w.create(&password(), mnemonic()).unwrap();
        let uri = w.stealth_uri().unwrap();
        assert!(
            uri.starts_with("st:pls:0x"),
            "default network is PulseChain: {uri}"
        );
        assert_eq!(uri.len(), "st:pls:0x".len() + 132);
        crate::security::stealth::StealthMetaAddress::parse(&uri).unwrap();
        w.lock();
        assert!(matches!(w.stealth_uri(), Err(WalletError::WalletLocked)));
    }
}
