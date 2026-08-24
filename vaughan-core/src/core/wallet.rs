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

    /// Active account index (HD or imported).
    pub fn active_account_index(&self) -> Result<u32, WalletError> {
        Ok(self.require_unlocked()?.active_index())
    }

    /// Display label for the active account (e.g. `wallet 0` or `W1-HD 1`).
    pub fn active_account_label(&self) -> Result<&str, WalletError> {
        Ok(self.require_unlocked()?.active_account().label.as_str())
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
        let persisted = self.persisted.as_ref().ok_or(WalletError::NotInitialized)?;
        let mut plaintext = decrypt(&persisted.vault, password)?;
        let phrase = std::str::from_utf8(&plaintext).map_err(|_| {
            WalletError::DecryptionFailed("vault did not contain a valid mnemonic".to_string())
        })?;
        let mut secrets = VaultSecrets::decode(phrase)?;
        plaintext.zeroize();
        let mut accounts =
            AccountManager::from_secrets(&secrets, AccountManager::DEFAULT_ACCOUNT_COUNT)?;
        secrets.zeroize();
        if accounts.set_active(persisted.active_account_index).is_err() {
            // Imported-only edge / stale index: fall back to first account.
            if let Some(first) = accounts.accounts().first().map(|a| a.index) {
                accounts.set_active(first)?;
            }
        }
        self.accounts = Some(accounts);
        Ok(())
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
    pub fn export_mnemonic(&self, password: &SecretString) -> Result<SecretString, WalletError> {
        self.require_unlocked()?;
        self.verify_password(password)?;
        Ok(self.require_unlocked()?.mnemonic_phrase())
    }

    /// Export the active account's private key (hex) after password confirmation.
    pub fn export_active_private_key(
        &self,
        password: &SecretString,
    ) -> Result<SecretString, WalletError> {
        let accounts = self.require_unlocked()?;
        self.verify_password(password)?;
        accounts.export_private_key(accounts.active_index())
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
        let parsed = url::Url::parse(rpc_url).map_err(|_| {
            WalletError::InvalidTransaction("RPC URL must be a valid http(s) URL".into())
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(WalletError::InvalidTransaction(
                "RPC URL must be http or https".into(),
            ));
        }
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
            rpc_url: rpc_url.to_string(),
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

    /// Snapshot RPC + account facts so the TUI can drop the wallet mutex before network I/O.
    ///
    /// Holding [`WalletState`] across `eth_getBalance` / gas RPCs freezes the UI on
    /// `[busy]` — chrome refresh must use this snapshot pattern instead.
    pub fn chrome_rpc_snapshot(&self) -> Result<ChromeRpcSnapshot, WalletError> {
        let (net, address) = self.active_context()?;
        Ok(ChromeRpcSnapshot {
            rpc_url: self.effective_rpc(),
            fallback_rpc_urls: net.fallback_rpc_urls.clone(),
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
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
        adapter.get_assets(address, &extras).await
    }

    /// Recent ERC-20 Transfer activity for the active account (newest first).
    pub async fn activity(&self, limit: u32) -> Result<Vec<crate::chains::TxRecord>, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
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
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;

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
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
        adapter.get_token_balance(token_address, address).await
    }

    /// Import an ERC-20 by contract address (reads on-chain metadata, persists).
    pub async fn import_custom_token(
        &mut self,
        token_address: &str,
    ) -> Result<CustomToken, WalletError> {
        let (net, _) = self.active_context()?;
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
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
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
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
    ) -> Result<TxHash, WalletError> {
        let (net, address) = self.active_context()?;
        let adapter = EvmAdapter::new(
            &self.effective_rpc(),
            net.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
        .await?;
        let service = TransactionService::new();
        let mut tx = service.build_erc20_transfer(address, token, to, amount, net.chain_id)?;
        let fee = service.estimate_fee(&adapter, &tx).await?;
        service.apply_fee(&mut tx, &fee)?;
        let ChainTransaction::Evm(evm_tx) = tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".into(),
            ));
        };
        self.send_transaction(evm_tx).await
    }

    /// ERC-20 transfer using an already-approved fee.
    pub async fn send_token_with_fee(
        &self,
        token: &str,
        to: &str,
        amount: &str,
        fee: &Fee,
    ) -> Result<TxHash, WalletError> {
        let (net, address) = self.active_context()?;
        let service = TransactionService::new();
        let mut tx = service.build_erc20_transfer(address, token, to, amount, net.chain_id)?;
        service.apply_fee(&mut tx, fee)?;
        let ChainTransaction::Evm(evm_tx) = tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".into(),
            ));
        };
        self.send_transaction(evm_tx).await
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

    /// Native transfer using an already-approved fee (e.g. Slow/Normal/Fast/Ape).
    ///
    /// Does not re-estimate; `fee` is applied verbatim so the UI confirmation
    /// matches what is signed.
    pub async fn send_with_fee(
        &self,
        to: &str,
        value_wei: &str,
        fee: &Fee,
    ) -> Result<TxHash, WalletError> {
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
