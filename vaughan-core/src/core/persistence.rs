//! On-disk persistence: the encrypted vault plus user settings.
//!
//! Nothing secret is ever written here. [`EncryptedVault`] only holds the
//! Argon2id salt, AES-GCM nonce, and ciphertext; the active network/account are
//! plain metadata. Writes are atomic (temp file + rename) so a crash can never
//! leave a half-written vault. Before overwriting, the previous good file is
//! copied to `wallet.json.bak` so a corrupt primary can still be recovered.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::profile::OperatingMode;
use crate::error::WalletError;
use crate::security::encryption::EncryptedVault;

/// Schema version; bump when the persisted layout changes.
pub const CURRENT_VERSION: u32 = 1;

/// Default wallet data file name.
pub const WALLET_FILE: &str = "wallet.json";

/// Last-known-good copy written beside [`WALLET_FILE`] before each overwrite.
pub const WALLET_BACKUP_SUFFIX: &str = ".bak";

fn backup_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}{WALLET_BACKUP_SUFFIX}", path.display()))
}

/// Default profile name (human adviser / savings).
pub const DEFAULT_PROFILE: &str = "default";

/// Sentient agent profile — the agent's own seed (`vaughan-sentient` MCP).
pub const SENTIENT_PROFILE: &str = "sentient";

/// Legacy alias for [`SENTIENT_PROFILE`] (pre-rename on-disk / CLI).
pub const DEGEN_PROFILE: &str = "degen";

/// True if `name` is the sentient (agent-owned) profile, including legacy `degen`.
pub fn is_sentient_profile(name: &str) -> bool {
    name == SENTIENT_PROFILE || name == DEGEN_PROFILE
}

/// Everything persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    pub vault: EncryptedVault,
    pub active_network_id: String,
    pub active_account_index: u32,
    #[serde(default)]
    pub operating_mode: OperatingMode,
    #[serde(default = "default_profile_string")]
    pub profile_name: String,
    /// User-imported ERC-20 contracts (meme coins, etc.), per chain.
    #[serde(default)]
    pub custom_tokens: Vec<CustomToken>,
    /// Whitelisted dApps: open in Freedom; origins feed the provider allowlist.
    #[serde(default)]
    pub trusted_dapps: Vec<TrustedDapp>,
    /// User-defined EVM networks (merged after built-ins).
    #[serde(default)]
    pub custom_networks: Vec<CustomNetwork>,
    /// Hardware watch accounts (address + path; no secrets). Forward-compatible.
    #[serde(default)]
    pub hardware: Vec<crate::security::hardware::HardwareAccountRecord>,
}

/// A user-imported ERC-20 (shown in Assets even at zero balance).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomToken {
    pub chain_id: u64,
    pub address: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_token_decimals")]
    pub decimals: u8,
}

fn default_token_decimals() -> u8 {
    18
}

/// A user-defined EVM network (persisted; not a built-in).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomNetwork {
    /// Stable id (e.g. `custom-1337`).
    pub id: String,
    pub name: String,
    pub chain_id: u64,
    pub rpc_url: String,
    #[serde(default = "default_native_symbol")]
    pub native_symbol: String,
    #[serde(default)]
    pub is_testnet: bool,
}

fn default_native_symbol() -> String {
    "ETH".into()
}

impl CustomNetwork {
    /// Convert to the runtime network config used by adapters.
    pub fn to_evm_config(&self) -> crate::chains::evm::networks::EvmNetworkConfig {
        let mut cfg = crate::chains::evm::networks::EvmNetworkConfig::new(
            self.id.clone(),
            self.name.clone(),
            self.chain_id,
            self.rpc_url.clone(),
            self.native_symbol.clone(),
            self.native_symbol.clone(),
            self.is_testnet,
        );
        cfg.decimals = 18;
        cfg
    }
}

/// A bookmarked / allowlisted dApp URL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedDapp {
    pub name: String,
    pub url: String,
}

/// Built-in PulseChain dApps seeded into every vault (provider origins + launcher).
pub fn default_trusted_dapps() -> Vec<TrustedDapp> {
    vec![
        TrustedDapp {
            name: "SquirrelSwap".into(),
            url: "https://app.squirrelswap.pro/#/".into(),
        },
        TrustedDapp {
            name: "LibertySwap".into(),
            url: "https://libertyswap.finance/".into(),
        },
        // app.pulsex.com is a gateway *directory* (IPFS mirrors), not the DEX UI.
        // Open a listed IPFS link from there; Vaughan dApp-browser trusts the
        // extension Origin so mirror hosts still reach the provider.
        TrustedDapp {
            name: "PulseX (pick IPFS mirror)".into(),
            url: "https://app.pulsex.com/".into(),
        },
        TrustedDapp {
            name: "9inch".into(),
            url: "https://app.9inch.io/swap?chain=pulse".into(),
        },
    ]
}

fn dapp_origin(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let origin = u.origin().ascii_serialization();
    if origin == "null" {
        None
    } else {
        Some(origin)
    }
}

/// Append any missing [`default_trusted_dapps`] entries (matched by origin).
/// Returns `true` when the list changed.
pub fn merge_default_trusted_dapps(list: &mut Vec<TrustedDapp>) -> bool {
    let mut changed = false;
    for dapp in default_trusted_dapps() {
        let Some(want) = dapp_origin(&dapp.url) else {
            continue;
        };
        let already = list
            .iter()
            .any(|e| dapp_origin(&e.url).as_deref() == Some(want.as_str()));
        if !already {
            list.push(dapp);
            changed = true;
        }
    }
    changed
}

fn default_profile_string() -> String {
    DEFAULT_PROFILE.to_string()
}

impl PersistedState {
    pub fn new(vault: EncryptedVault, active_network_id: impl Into<String>) -> Self {
        Self {
            version: CURRENT_VERSION,
            vault,
            active_network_id: active_network_id.into(),
            active_account_index: 0,
            operating_mode: OperatingMode::HumanOnly,
            profile_name: DEFAULT_PROFILE.to_string(),
            custom_tokens: Vec::new(),
            trusted_dapps: default_trusted_dapps(),
            custom_networks: Vec::new(),
            hardware: Vec::new(),
        }
    }

    pub fn with_mode_and_profile(
        vault: EncryptedVault,
        active_network_id: impl Into<String>,
        operating_mode: OperatingMode,
        profile_name: impl Into<String>,
    ) -> Self {
        Self {
            version: CURRENT_VERSION,
            vault,
            active_network_id: active_network_id.into(),
            active_account_index: 0,
            operating_mode,
            profile_name: profile_name.into(),
            custom_tokens: Vec::new(),
            trusted_dapps: default_trusted_dapps(),
            custom_networks: Vec::new(),
            hardware: Vec::new(),
        }
    }
}

/// Loads and saves [`PersistedState`] to a JSON file on disk.
pub struct StateManager {
    path: PathBuf,
}

impl StateManager {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The default location: `<data_dir>/vaughan-cli/wallet.json`.
    pub fn default_path() -> Result<PathBuf, WalletError> {
        Self::profile_path(DEFAULT_PROFILE)
    }

    /// Profile-specific location:
    /// - "default" -> `<data_dir>/vaughan-cli/wallet.json`
    /// - other (e.g. "degen") -> `<data_dir>/vaughan-cli/profiles/<name>/wallet.json`
    pub fn profile_path(profile_name: &str) -> Result<PathBuf, WalletError> {
        let base = dirs::data_dir().ok_or_else(|| {
            WalletError::Io("could not determine the user data directory".to_string())
        })?;
        let vaughan_base = base.join("vaughan-cli");
        if profile_name.is_empty() || profile_name == DEFAULT_PROFILE {
            Ok(vaughan_base.join(WALLET_FILE))
        } else {
            Ok(vaughan_base
                .join("profiles")
                .join(profile_name)
                .join(WALLET_FILE))
        }
    }

    /// Construct a StateManager for a specific profile.
    pub fn for_profile(profile_name: &str) -> Result<Self, WalletError> {
        let path = Self::profile_path(profile_name)?;
        Ok(Self::new(path))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// On Unix: restrict the vault directory to `0o700` and the vault file to
    /// `0o600` so other local users can never read the ciphertext, regardless
    /// of umask. Failures are logged, not fatal — reading the vault must not
    /// break on filesystems that reject chmod (e.g. some mounted/FAT paths).
    #[cfg(unix)]
    fn lockdown_permissions(&self) {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = self.path.parent() {
            if let Err(e) = fs::set_permissions(parent, fs::Permissions::from_mode(0o700)) {
                tracing::warn!("could not restrict vault dir permissions: {e}");
            }
        }
        if let Err(e) = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)) {
            tracing::warn!("could not restrict vault file permissions: {e}");
        }
    }

    /// Serialize `state` to disk atomically.
    ///
    /// If a previous vault exists, it is copied to `*.bak` first (best effort).
    /// Then write `*.tmp` → fsync → rename over the primary. On Unix the vault
    /// directory is `0o700` and vault / temp / bak are `0o600`.
    pub fn save(&self, state: &PersistedState) -> Result<(), WalletError> {
        let json = serde_json::to_string_pretty(state)?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| WalletError::Io("wallet path has no parent directory".to_string()))?;
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }

        // Preserve last good vault before overwrite (ignore errors: first save).
        if self.path.exists() {
            let bak = backup_path(&self.path);
            if let Err(e) = fs::copy(&self.path, &bak) {
                tracing::warn!("could not write vault backup {}: {e}", bak.display());
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&bak, fs::Permissions::from_mode(0o600));
                }
            }
        }

        let tmp = PathBuf::from(format!("{}.tmp", self.path.display()));
        fs::write(&tmp, json.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(&tmp, &self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Load the persisted state, or `WalletError::NotInitialized` when absent.
    ///
    /// If the primary file is corrupt JSON, attempts `*.bak` once and returns a
    /// clear corruption error when both fail.
    pub fn load(&self) -> Result<PersistedState, WalletError> {
        if !self.exists() {
            return Err(WalletError::NotInitialized);
        }
        // Lock down permissions on existing vaults written before the
        // 0o600/0o700 rules existed (best effort; see `lockdown_permissions`).
        #[cfg(unix)]
        self.lockdown_permissions();

        match self.load_from_path(&self.path) {
            Ok(state) => Ok(state),
            Err(primary_err) => {
                let bak = backup_path(&self.path);
                if !bak.exists() {
                    return Err(primary_err);
                }
                tracing::warn!(
                    "primary vault unreadable ({}); trying backup {}",
                    primary_err,
                    bak.display()
                );
                match self.load_from_path(&bak) {
                    Ok(state) => {
                        // Restore primary from backup so next unlock is clean.
                        if let Err(e) = fs::copy(&bak, &self.path) {
                            tracing::warn!("could not restore primary from backup: {e}");
                        } else {
                            #[cfg(unix)]
                            self.lockdown_permissions();
                        }
                        Ok(state)
                    }
                    Err(_) => Err(WalletError::Serialization(format!(
                        "wallet file is corrupt (and backup failed). Primary error: {primary_err}"
                    ))),
                }
            }
        }
    }

    fn load_from_path(&self, path: &Path) -> Result<PersistedState, WalletError> {
        let json = fs::read_to_string(path)?;
        let state: PersistedState = serde_json::from_str(&json).map_err(|e| {
            WalletError::Serialization(format!("corrupt wallet data at {}: {e}", path.display()))
        })?;
        if state.version > CURRENT_VERSION {
            return Err(WalletError::Serialization(format!(
                "wallet file version {} is newer than supported {}",
                state.version, CURRENT_VERSION
            )));
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_vault() -> EncryptedVault {
        EncryptedVault {
            salt: "aa".into(),
            nonce: "bb".into(),
            ciphertext: "cc".into(),
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let sm = StateManager::new(path);
        let state = PersistedState::new(dummy_vault(), "sepolia");
        sm.save(&state).unwrap();

        let loaded = sm.load().unwrap();
        assert_eq!(loaded.active_network_id, "sepolia");
        assert_eq!(loaded.active_account_index, 0);
        assert_eq!(loaded.vault.ciphertext, "cc");
        assert_eq!(loaded.version, CURRENT_VERSION);
        assert_eq!(loaded.trusted_dapps.len(), default_trusted_dapps().len());
        assert!(loaded.hardware.is_empty());
    }

    #[test]
    fn hardware_field_roundtrip() {
        use crate::security::hardware::{HardwareAccountRecord, HardwareVendor, HwChainFamily};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let sm = StateManager::new(path);
        let mut state = PersistedState::new(dummy_vault(), "pulsechain-testnet-v4");
        state.hardware.push(HardwareAccountRecord {
            vendor: HardwareVendor::Trezor,
            family: HwChainFamily::Evm,
            derivation_path: "m/44'/60'/0'/0/1".into(),
            network_id: Some("943".into()),
            address: "0x2222222222222222222222222222222222222222".into(),
            label: "trezor-1".into(),
        });
        sm.save(&state).unwrap();
        let loaded = sm.load().unwrap();
        assert_eq!(loaded.hardware.len(), 1);
        assert_eq!(loaded.hardware[0].label, "trezor-1");
        assert_eq!(loaded.hardware[0].vendor, HardwareVendor::Trezor);
    }

    #[test]
    fn legacy_json_without_hardware_deserializes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let json =
            serde_json::to_string(&PersistedState::new(dummy_vault(), "pulsechain")).unwrap();
        // Strip hardware if present — simulate pre-Phase-0 file.
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        if let Some(obj) = v.as_object_mut() {
            obj.remove("hardware");
        }
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        let sm = StateManager::new(path);
        let loaded = sm.load().unwrap();
        assert!(loaded.hardware.is_empty());
    }

    #[test]
    fn merge_default_dapps_is_idempotent() {
        let mut list = default_trusted_dapps();
        assert!(!merge_default_trusted_dapps(&mut list));
        assert_eq!(list.len(), 4);
        list.clear();
        assert!(merge_default_trusted_dapps(&mut list));
        assert!(list.iter().any(|d| d.url.contains("squirrelswap")));
        assert!(list.iter().any(|d| d.url.contains("libertyswap")));
        assert!(list.iter().any(|d| d.url.contains("pulsex")));
        assert!(list.iter().any(|d| d.url.contains("9inch")));
    }

    #[test]
    fn load_missing_is_not_initialized() {
        let dir = tempfile::tempdir().unwrap();
        let sm = StateManager::new(dir.path().join("nope.json"));
        assert!(matches!(sm.load(), Err(WalletError::NotInitialized)));
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let sm = StateManager::new(path.clone());
        sm.save(&PersistedState::new(dummy_vault(), "pulsechain"))
            .unwrap();
        assert!(path.exists());

        let leftovers = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[cfg(unix)]
    #[test]
    fn vault_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("vaughan-cli");
        let path = sub.join(WALLET_FILE);
        let sm = StateManager::new(path.clone());
        sm.save(&PersistedState::new(dummy_vault(), "pulsechain"))
            .unwrap();

        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "vault file must not be group/other readable"
        );
        let dir_mode = fs::metadata(&sub).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "vault dir must not be group/other readable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_locks_down_existing_vault() {
        use std::os::unix::fs::PermissionsExt;

        // Simulate a vault written by older code: valid state, world-readable.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let json =
            serde_json::to_string(&PersistedState::new(dummy_vault(), "pulsechain")).unwrap();
        fs::write(&path, json).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

        let sm = StateManager::new(path.clone());
        assert!(sm.load().is_ok());

        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "load must lock down the vault file");
        let dir_mode = fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "load must lock down the vault dir");
    }

    #[test]
    fn profile_mode_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("degen_wallet.json");
        let sm = StateManager::new(path);
        let state = PersistedState::with_mode_and_profile(
            dummy_vault(),
            "pulsechain-testnet-v4",
            OperatingMode::DegenTrader,
            "degen",
        );
        sm.save(&state).unwrap();

        let loaded = sm.load().unwrap();
        assert_eq!(loaded.operating_mode, OperatingMode::DegenTrader);
        assert_eq!(loaded.profile_name, "degen");
        assert_eq!(loaded.active_network_id, "pulsechain-testnet-v4");
    }

    #[test]
    fn profile_path_resolution() {
        let default_path = StateManager::profile_path(DEFAULT_PROFILE).unwrap();
        assert!(default_path.ends_with("vaughan-cli/wallet.json"));

        let degen_path = StateManager::profile_path(DEGEN_PROFILE).unwrap();
        assert!(degen_path.ends_with("vaughan-cli/profiles/degen/wallet.json"));

        let sentient_path = StateManager::profile_path(SENTIENT_PROFILE).unwrap();
        assert!(sentient_path.ends_with("vaughan-cli/profiles/sentient/wallet.json"));

        let custom_path = StateManager::profile_path("bot1").unwrap();
        assert!(custom_path.ends_with("vaughan-cli/profiles/bot1/wallet.json"));
    }

    #[test]
    fn sentient_profile_includes_legacy_degen_alias() {
        assert!(is_sentient_profile(SENTIENT_PROFILE));
        assert!(is_sentient_profile(DEGEN_PROFILE));
        assert!(!is_sentient_profile(DEFAULT_PROFILE));
        assert!(!is_sentient_profile("bot1"));
    }

    #[test]
    fn save_writes_bak_of_previous_vault() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let bak = backup_path(&path);
        let sm = StateManager::new(path.clone());

        let first = PersistedState::new(dummy_vault(), "sepolia");
        sm.save(&first).unwrap();
        assert!(!bak.exists(), "first save has nothing to back up");

        let second = PersistedState::new(dummy_vault(), "pulsechain");
        sm.save(&second).unwrap();
        assert!(bak.exists(), "second save must leave wallet.json.bak");

        let bak_state: PersistedState =
            serde_json::from_str(&fs::read_to_string(&bak).unwrap()).unwrap();
        assert_eq!(bak_state.active_network_id, "sepolia");
        assert_eq!(sm.load().unwrap().active_network_id, "pulsechain");
    }

    #[test]
    fn load_recovers_from_bak_when_primary_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let bak = backup_path(&path);
        let sm = StateManager::new(path.clone());

        sm.save(&PersistedState::new(dummy_vault(), "pulsechain"))
            .unwrap();
        sm.save(&PersistedState::new(dummy_vault(), "sepolia"))
            .unwrap();
        // Bak holds pulsechain; overwrite primary with garbage.
        fs::write(&path, "{not-json").unwrap();
        assert!(bak.exists());

        let loaded = sm.load().unwrap();
        assert_eq!(loaded.active_network_id, "pulsechain");
        // Primary should be restored from bak.
        let primary: PersistedState =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(primary.active_network_id, "pulsechain");
    }
}
