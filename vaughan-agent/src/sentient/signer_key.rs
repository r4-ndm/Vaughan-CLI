//! Sentient signer key management and queue HMAC key derivation.
//!
//! The signer key (`signer.key`) is a 0600 file owned by the current UID
//! created under the profile dir on `preset apply`.
//! The sentient queue HMAC secret is derived from this key rather than using
//! the loopback session token (`mcp.session`).

use std::fs;
use std::path::{Path, PathBuf};

use rand::RngCore;
use sha2::{Digest, Sha256};
use vaughan_core::core::proposal::{
    verify_proposal_hmac_sentient, McpSessionToken, ProposalError, QueuedProposal,
};

use crate::error::AgentError;
use crate::sentient::file_guard::assert_owner_only_file;

/// Signer key filename under the profile directory.
pub const SIGNER_KEY_FILE: &str = "signer.key";

/// Derive the sentient queue HMAC key from the raw signer key using a domain separator.
pub fn derive_sentient_queue_key(signer_key: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"vaughan-sentient-queue-hmac-v1:\n");
    hasher.update(signer_key);
    hasher.finalize().into()
}

/// Initialize the signer key file under `profile_dir` with mode 0600.
///
/// If `signer.key` does not exist, generates 32 cryptographically secure random bytes
/// and writes them as hex. Ensures mode 0600 and process UID ownership.
pub fn init_signer_key(profile_dir: &Path) -> Result<PathBuf, AgentError> {
    crate::paths::assert_signer_store_writable(profile_dir)?;
    fs::create_dir_all(profile_dir)
        .map_err(|e| AgentError::ProviderError(format!("create profile dir: {e}")))?;
    let path = profile_dir.join(SIGNER_KEY_FILE);
    if !path.exists() {
        let mut key_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key_bytes);
        let hex_key = hex::encode(key_bytes);
        let tmp = profile_dir.join(format!(".{SIGNER_KEY_FILE}.tmp"));
        {
            use std::io::Write;
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp).map_err(|e| {
                AgentError::ProviderError(format!("open tmp signer key {}: {e}", tmp.display()))
            })?;
            f.write_all(hex_key.as_bytes()).map_err(|e| {
                AgentError::ProviderError(format!("write tmp signer key {}: {e}", tmp.display()))
            })?;
        }
        fs::rename(&tmp, &path).map_err(|e| {
            AgentError::ProviderError(format!(
                "rename {} -> {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }

    assert_owner_only_file(&path)?;
    Ok(path)
}

/// Read the raw signer key from `profile_dir/signer.key`, asserting 0600 mode and UID ownership.
pub fn load_signer_key(profile_dir: &Path) -> Result<Vec<u8>, AgentError> {
    let path = profile_dir.join(SIGNER_KEY_FILE);
    if !path.exists() {
        return Err(AgentError::SecurityViolation(format!(
            "signer key file missing at {} — run `vaughan --profile sentient preset apply <preset>` first",
            path.display()
        )));
    }
    assert_owner_only_file(&path)?;
    let content = fs::read(&path).map_err(|e| {
        AgentError::ProviderError(format!("failed to read {}: {e}", path.display()))
    })?;
    let trimmed = {
        let mut s = content.as_slice();
        while let Some((first, rest)) = s.split_first() {
            if first.is_ascii_whitespace() {
                s = rest;
            } else {
                break;
            }
        }
        while let Some((last, rest)) = s.split_last() {
            if last.is_ascii_whitespace() {
                s = rest;
            } else {
                break;
            }
        }
        s
    };
    if trimmed.is_empty() {
        return Err(AgentError::SecurityViolation(format!(
            "signer key file at {} is empty",
            path.display()
        )));
    }
    Ok(trimmed.to_vec())
}

/// Load the signer key and derive the 32-byte sentient queue HMAC key.
pub fn sentient_queue_hmac_key(profile_dir: &Path) -> Result<[u8; 32], AgentError> {
    let raw = load_signer_key(profile_dir)?;
    Ok(derive_sentient_queue_key(&raw))
}

/// Verify a queued proposal for a sentient profile.
///
/// Fails closed if signed with the old session token, returning an error whose reason
/// explicitly names the key change.
pub fn verify_sentient_queue_entry(
    queued: &QueuedProposal,
    profile_dir: &Path,
) -> Result<(), ProposalError> {
    let queue_secret = sentient_queue_hmac_key(profile_dir)
        .map_err(|e| ProposalError::Io(format!("failed to load sentient queue key: {e}")))?;
    let legacy_token = McpSessionToken::read(profile_dir)
        .ok()
        .flatten()
        .map(|s| s.into_bytes());
    verify_proposal_hmac_sentient(&queue_secret, legacy_token.as_deref(), queued)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_init_and_load_signer_key() {
        let dir = tempdir().unwrap();
        let path = init_signer_key(dir.path()).unwrap();
        assert!(path.is_file());

        let key = load_signer_key(dir.path()).unwrap();
        assert_eq!(key.len(), 64); // hex-encoded 32 bytes

        let derived = sentient_queue_hmac_key(dir.path()).unwrap();
        assert_ne!(derived, [0u8; 32]);
    }

    #[cfg(unix)]
    #[test]
    fn test_signer_key_mode_0644_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        init_signer_key(dir.path()).unwrap();
        let path = dir.path().join(SIGNER_KEY_FILE);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let err = load_signer_key(dir.path()).unwrap_err();
        assert!(err.to_string().contains("0644"));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load_signer_key(dir.path()).is_ok());
    }
}
