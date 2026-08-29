//! Provider loopback session token (native-parity Trick 1).
//!
//! Written `0o600` beside the profile vault. Every provider client must present
//! it (`?access_token=…` or `Authorization: Bearer …`) — the token is required
//! for all origins because the `Origin` header is forgeable by any local
//! process.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::WalletError;

/// File name under the profile directory.
pub const PROVIDER_SESSION_FILE: &str = "provider.session";

/// Ephemeral shared secret between Vaughan and trusted local clients.
pub struct ProviderSessionToken {
    token: String,
}

impl std::fmt::Debug for ProviderSessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderSessionToken(REDACTED)")
    }
}

impl Clone for ProviderSessionToken {
    fn clone(&self) -> Self {
        Self {
            token: self.token.clone(),
        }
    }
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

    /// Wrap an existing token value (e.g. the running server's live slot) so
    /// it can be published to another profile dir on a profile switch.
    pub fn from_string(token: String) -> Self {
        Self { token }
    }

    /// Hex token string (never log this).
    pub fn as_str(&self) -> &str {
        &self.token
    }

    /// Path of the session file under `profile_dir`.
    pub fn path(profile_dir: &Path) -> PathBuf {
        profile_dir.join(PROVIDER_SESSION_FILE)
    }

    /// Persist with restrictive permissions (0600 from creation — no
    /// permissive write-then-chmod window).
    pub fn write(&self, profile_dir: &Path) -> Result<(), WalletError> {
        fs::create_dir_all(profile_dir).map_err(|e| WalletError::Io(e.to_string()))?;
        let path = Self::path(profile_dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true).mode(0o600);
            let mut f = opts
                .open(&path)
                .map_err(|e| WalletError::Io(e.to_string()))?;
            use std::io::Write;
            f.write_all(self.token.as_bytes())
                .map_err(|e| WalletError::Io(e.to_string()))?;
        }
        #[cfg(not(unix))]
        {
            fs::write(&path, self.token.as_bytes()).map_err(|e| WalletError::Io(e.to_string()))?;
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
