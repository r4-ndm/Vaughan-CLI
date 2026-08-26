//! Provider loopback session token (native-parity Trick 1).
//!
//! Written `0o600` beside the profile vault. The Chromium extension must present
//! it on the WebSocket URL (`?access_token=…`). Freedom may keep Origin-only
//! until its transport reads this file; set
//! `VAUGHAN_PROVIDER_REQUIRE_TOKEN=1` to require the token for every client.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::WalletError;

/// File name under the profile directory.
pub const PROVIDER_SESSION_FILE: &str = "provider.session";

/// Ephemeral shared secret between Vaughan and trusted local clients.
#[derive(Debug, Clone)]
pub struct ProviderSessionToken {
    token: String,
}

impl ProviderSessionToken {
    /// Generate a fresh 256-bit hex token.
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self {
            token: hex::encode(bytes),
        }
    }

    /// Hex token string (never log this).
    pub fn as_str(&self) -> &str {
        &self.token
    }

    /// Path of the session file under `profile_dir`.
    pub fn path(profile_dir: &Path) -> PathBuf {
        profile_dir.join(PROVIDER_SESSION_FILE)
    }

    /// Persist with restrictive permissions.
    pub fn write(&self, profile_dir: &Path) -> Result<(), WalletError> {
        fs::create_dir_all(profile_dir).map_err(|e| WalletError::Io(e.to_string()))?;
        let path = Self::path(profile_dir);
        fs::write(&path, self.token.as_bytes()).map_err(|e| WalletError::Io(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Read an existing token, if any.
    pub fn read(profile_dir: &Path) -> Result<Option<String>, WalletError> {
        let path = Self::path(profile_dir);
        match fs::read_to_string(&path) {
            Ok(s) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(WalletError::Io(e.to_string())),
        }
    }

    /// Remove the session file (lock / shutdown).
    pub fn invalidate(profile_dir: &Path) -> Result<(), WalletError> {
        let path = Self::path(profile_dir);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(WalletError::Io(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_token_file() {
        let dir = tempdir().unwrap();
        let t = ProviderSessionToken::generate();
        assert_eq!(t.as_str().len(), 64);
        t.write(dir.path()).unwrap();
        let read = ProviderSessionToken::read(dir.path()).unwrap().unwrap();
        assert_eq!(read, t.as_str());
        ProviderSessionToken::invalidate(dir.path()).unwrap();
        assert!(ProviderSessionToken::read(dir.path()).unwrap().is_none());
    }
}
