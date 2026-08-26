//! Persistent dApp site grants (origins approved via `eth_requestAccounts`).
//!
//! Grants are origin strings only — no secret material. They are stored
//! `0o600` beside the profile vault so a TUI restart does not force every
//! previously-approved dApp to reconnect (MetaMask persists site permissions
//! the same way). An explicit wallet lock clears the file: grants remain
//! scoped to the user's intent, and signing still requires a fresh per-request
//! approval regardless of a stored grant.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::WalletError;

/// File name under the profile directory.
pub const SITE_GRANTS_FILE: &str = "site-grants.json";

/// Path of the grants file under `profile_dir`.
pub fn path(profile_dir: &Path) -> PathBuf {
    profile_dir.join(SITE_GRANTS_FILE)
}

/// Load granted origins; an absent or empty file yields an empty set.
pub fn load(profile_dir: &Path) -> Result<HashSet<String>, WalletError> {
    let path = path(profile_dir);
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| WalletError::Serialization(format!("site grants corrupt: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(e) => Err(WalletError::Io(e.to_string())),
    }
}

/// Persist the granted origins with restrictive permissions.
pub fn save(profile_dir: &Path, sites: &HashSet<String>) -> Result<(), WalletError> {
    fs::create_dir_all(profile_dir).map_err(|e| WalletError::Io(e.to_string()))?;
    let raw = serde_json::to_string_pretty(&sites.iter().collect::<Vec<_>>())
        .map_err(|e| WalletError::Serialization(e.to_string()))?;
    let path = path(profile_dir);
    fs::write(&path, raw).map_err(|e| WalletError::Io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Remove the grants file (explicit wallet lock).
pub fn clear(profile_dir: &Path) -> Result<(), WalletError> {
    match fs::remove_file(path(profile_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WalletError::Io(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_grants_file() {
        let dir = tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_empty());

        let mut sites = HashSet::new();
        sites.insert("https://dapp.example".to_string());
        sites.insert("chrome-extension://abc".to_string());
        save(dir.path(), &sites).unwrap();
        assert_eq!(load(dir.path()).unwrap(), sites);

        clear(dir.path()).unwrap();
        assert!(load(dir.path()).unwrap().is_empty());
        // Clearing twice is not an error.
        clear(dir.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn grants_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let mut sites = HashSet::new();
        sites.insert("https://dapp.example".to_string());
        save(dir.path(), &sites).unwrap();
        let mode = fs::metadata(path(dir.path())).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
