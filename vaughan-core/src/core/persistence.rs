//! On-disk persistence: the encrypted vault plus user settings.
//!
//! Nothing secret is ever written here. [`EncryptedVault`] only holds the
//! Argon2id salt, AES-GCM nonce, and ciphertext; the active network/account are
//! plain metadata. Writes are atomic (temp file + rename) so a crash can never
//! leave a half-written vault.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::WalletError;
use crate::security::encryption::EncryptedVault;

/// Schema version; bump when the persisted layout changes.
pub const CURRENT_VERSION: u32 = 1;

/// Default wallet data file name.
pub const WALLET_FILE: &str = "wallet.json";

/// Everything persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    pub vault: EncryptedVault,
    pub active_network_id: String,
    pub active_account_index: u32,
}

impl PersistedState {
    pub fn new(vault: EncryptedVault, active_network_id: impl Into<String>) -> Self {
        Self {
            version: CURRENT_VERSION,
            vault,
            active_network_id: active_network_id.into(),
            active_account_index: 0,
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
        let base = dirs::data_dir().ok_or_else(|| {
            WalletError::Io("could not determine the user data directory".to_string())
        })?;
        Ok(base.join("vaughan-cli").join(WALLET_FILE))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Serialize `state` to disk atomically.
    pub fn save(&self, state: &PersistedState) -> Result<(), WalletError> {
        let json = serde_json::to_string_pretty(state)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", self.path.display()));
        fs::write(&tmp, json.as_bytes())?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Load the persisted state, or `WalletError::NotInitialized` when absent.
    pub fn load(&self) -> Result<PersistedState, WalletError> {
        if !self.exists() {
            return Err(WalletError::NotInitialized);
        }
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
}
