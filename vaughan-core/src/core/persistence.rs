//! On-disk persistence: the encrypted vault plus user settings.
//!
//! Nothing secret is ever written here. [`EncryptedVault`] only holds the
//! Argon2id salt, AES-GCM nonce, and ciphertext; the active network/account are
//! plain metadata. Writes are atomic (temp file + rename) so a crash can never
//! leave a half-written vault.

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

/// Default profile name.
pub const DEFAULT_PROFILE: &str = "default";

/// Isolated degen profile name.
pub const DEGEN_PROFILE: &str = "degen";

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
    /// On Unix the vault directory is created `0o700` and the vault (and its
    /// temp file) are written `0o600` so other local users can never read the
    /// ciphertext, regardless of umask.
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
    pub fn load(&self) -> Result<PersistedState, WalletError> {
        if !self.exists() {
            return Err(WalletError::NotInitialized);
        }
        // Lock down permissions on existing vaults written before the
        // 0o600/0o700 rules existed (best effort; see `lockdown_permissions`).
        #[cfg(unix)]
        self.lockdown_permissions();
        let json = fs::read_to_string(&self.path)?;
        let state: PersistedState = serde_json::from_str(&json)?;
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

        let custom_path = StateManager::profile_path("bot1").unwrap();
        assert!(custom_path.ends_with("vaughan-cli/profiles/bot1/wallet.json"));
    }
}
