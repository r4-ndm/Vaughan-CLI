//! Profile directory helpers and isolated signer store resolution.
//!
//! # Signer Store Resolution
//! In isolated signer mode, fund-safety critical files (`wallet.json`,
//! `sentient-policy.toml`, `sentient-spend.json`, `stealth-pending.json`,
//! `dca-plans.json`, `proposals/`) live in a dedicated store owned by the
//! isolated signer daemon process (mode 0700 or 0750).
//!
//! Precedence order for resolution:
//! 1. Explicit `--store-root` (tests / CLI)
//! 2. `VAUGHAN_SIGNER_STORE` environment variable
//! 3. Unconfigured (fails closed when signer mode is active; falls back to legacy
//!    profile dir only when signer mode is OFF).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI8, Ordering};

use crate::error::AgentError;

/// Environment variable specifying the isolated signer store directory.
pub const ENV_SIGNER_STORE: &str = "VAUGHAN_SIGNER_STORE";

/// Environment variable enabling signer mode (`1`, `true`, `yes`, `on`).
pub const ENV_SIGNER_MODE: &str = "VAUGHAN_SIGNER_MODE";

/// Internal override for testing signer mode in-process without env var race conditions.
/// -1 = unset (use env vars), 0 = force false, 1 = force true.
static SIGNER_MODE_OVERRIDE: AtomicI8 = AtomicI8::new(-1);

/// Set or clear in-process override for signer mode (useful for unit tests).
pub fn set_signer_mode_override(mode: Option<bool>) {
    let val = match mode {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    };
    SIGNER_MODE_OVERRIDE.store(val, Ordering::SeqCst);
}

/// Check whether isolated signer mode is currently active.
///
/// Returns true if:
/// 1. An in-process override is set to true.
/// 2. `VAUGHAN_SIGNER_MODE` is set to "1", "true", "yes", or "on".
/// 3. `VAUGHAN_SIGNER_STORE` is set and non-empty.
pub fn is_signer_mode_active() -> bool {
    let override_val = SIGNER_MODE_OVERRIDE.load(Ordering::SeqCst);
    if override_val == 1 {
        return true;
    }
    if override_val == 0 {
        return false;
    }

    if let Ok(val) = std::env::var(ENV_SIGNER_MODE) {
        let v = val.trim().to_ascii_lowercase();
        if v == "1" || v == "true" || v == "yes" || v == "on" {
            return true;
        }
    }
    if let Ok(val) = std::env::var(ENV_SIGNER_STORE) {
        if !val.trim().is_empty() {
            return true;
        }
    }
    false
}

/// Directory that contains `wallet.json` for a profile.
///
/// Kept for backward compatibility when signer mode is OFF.
pub fn profile_dir(wallet_path: &Path) -> PathBuf {
    wallet_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Errors arising during signer store resolution and validation.
#[derive(Debug, thiserror::Error)]
pub enum SignerStoreError {
    #[error("signer_store_unavailable: directory `{0}` does not exist")]
    NotFound(PathBuf),

    #[error("signer_store_unavailable: path `{0}` is not a directory")]
    NotADirectory(PathBuf),

    #[error(
        "signer_store_unavailable: insecure permissions {mode:04o} on `{path}` (expected 0700 or 0750) — fix with `chmod 0700 {path}`",
        path = .path.display()
    )]
    InsecurePermissions { path: PathBuf, mode: u32 },

    #[error(
        "signer_store_unavailable: insecure ownership on `{path}`: owned by UID {owner_uid}, expected UID {expected_uid} — fix with `chown {expected_uid} {path}`",
        path = .path.display()
    )]
    InsecureOwnership {
        path: PathBuf,
        owner_uid: u32,
        expected_uid: u32,
    },

    #[error("signer_store_unavailable: signer store is unconfigured — set {ENV_SIGNER_STORE}")]
    Unconfigured,

    #[error(
        "signer_store_unavailable: writes to legacy profile directory `{path}` are refused when signer mode is active — signer store must be used",
        path = .path.display()
    )]
    LegacyProfileDirRefused { path: PathBuf },

    #[error("signer_store_unavailable: IO error accessing `{path}`: {source}", path = .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// First-class isolated signer store owned by the signer process UID.
///
/// Validates directory existence, permissions (0700 or 0750 on Unix), and process UID
/// ownership to ensure fund-safety critical state lives outside the agent's blast radius.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerStore {
    root: PathBuf,
    expected_uid: Option<u32>,
}

impl SignerStore {
    /// Open and validate a signer store directory against the current process UID.
    ///
    /// The directory must exist, have mode 0700 or 0750 on Unix, and be owned by the
    /// current process UID. Fails closed with an actionable error.
    pub fn open(store_root: &Path) -> Result<Self, SignerStoreError> {
        Self::open_with_expected_uid(store_root, None)
    }

    /// Open and validate a signer store directory against an explicit expected UID.
    pub fn open_with_expected_uid(
        store_root: &Path,
        expected_uid: Option<u32>,
    ) -> Result<Self, SignerStoreError> {
        if !store_root.exists() {
            return Err(SignerStoreError::NotFound(store_root.to_path_buf()));
        }
        if !store_root.is_dir() {
            return Err(SignerStoreError::NotADirectory(store_root.to_path_buf()));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = fs::metadata(store_root).map_err(|e| SignerStoreError::Io {
                path: store_root.to_path_buf(),
                source: e,
            })?;

            let mode = meta.mode() & 0o777;
            if mode != 0o700 && mode != 0o750 {
                return Err(SignerStoreError::InsecurePermissions {
                    path: store_root.to_path_buf(),
                    mode,
                });
            }

            let owner_uid = meta.uid();
            let target_uid = expected_uid.unwrap_or_else(|| rustix::process::getuid().as_raw());
            if owner_uid != target_uid {
                return Err(SignerStoreError::InsecureOwnership {
                    path: store_root.to_path_buf(),
                    owner_uid,
                    expected_uid: target_uid,
                });
            }
        }

        Ok(Self {
            root: store_root.to_path_buf(),
            expected_uid,
        })
    }

    /// Resolve the signer store following precedence:
    /// explicit `store_root` (tests/CLI) > `VAUGHAN_SIGNER_STORE` env > unconfigured.
    ///
    /// If signer mode is active and the store is missing or invalid, fails closed with
    /// `SignerStoreError`. If signer mode is inactive and no store is specified, returns `Ok(None)`.
    pub fn resolve(store_root: Option<&Path>) -> Result<Option<Self>, SignerStoreError> {
        Self::resolve_with_expected_uid(store_root, None)
    }

    /// Resolve the signer store with an optional expected UID override.
    pub fn resolve_with_expected_uid(
        store_root: Option<&Path>,
        expected_uid: Option<u32>,
    ) -> Result<Option<Self>, SignerStoreError> {
        if let Some(explicit) = store_root {
            return Self::open_with_expected_uid(explicit, expected_uid).map(Some);
        }
        if let Ok(env_val) = std::env::var(ENV_SIGNER_STORE) {
            let trimmed = env_val.trim();
            if !trimmed.is_empty() {
                return Self::open_with_expected_uid(Path::new(trimmed), expected_uid).map(Some);
            }
        }
        if is_signer_mode_active() {
            return Err(SignerStoreError::Unconfigured);
        }
        Ok(None)
    }

    /// Store root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Expected owner UID (if explicitly configured).
    pub fn expected_uid(&self) -> Option<u32> {
        self.expected_uid
    }

    /// Path to `wallet.json`.
    pub fn wallet_path(&self) -> PathBuf {
        self.root.join("wallet.json")
    }

    /// Alias for [`Self::wallet_path`].
    pub fn wallet_json(&self) -> PathBuf {
        self.wallet_path()
    }

    /// Path to `sentient-policy.toml`.
    pub fn policy_path(&self) -> PathBuf {
        self.root.join("sentient-policy.toml")
    }

    /// Alias for [`Self::policy_path`].
    pub fn sentient_policy_toml(&self) -> PathBuf {
        self.policy_path()
    }

    /// Path to `sentient-spend.json`.
    pub fn spend_path(&self) -> PathBuf {
        self.root.join("sentient-spend.json")
    }

    /// Alias for [`Self::spend_path`].
    pub fn sentient_spend_json(&self) -> PathBuf {
        self.spend_path()
    }

    /// Path to `sentient-spend.lock`.
    pub fn spend_lock_path(&self) -> PathBuf {
        self.root.join("sentient-spend.lock")
    }

    /// Alias for [`Self::spend_lock_path`].
    pub fn sentient_spend_lock(&self) -> PathBuf {
        self.spend_lock_path()
    }

    /// Path to `proposals/` audit directory.
    pub fn proposals_dir(&self) -> PathBuf {
        self.root.join("proposals")
    }

    /// Alias for [`Self::proposals_dir`].
    pub fn proposals(&self) -> PathBuf {
        self.proposals_dir()
    }

    /// Path to `stealth-pending.json`.
    pub fn stealth_pending_path(&self) -> PathBuf {
        self.root.join("stealth-pending.json")
    }

    /// Alias for [`Self::stealth_pending_path`].
    pub fn stealth_pending_json(&self) -> PathBuf {
        self.stealth_pending_path()
    }

    /// Path to `dca-plans.json`.
    pub fn dca_plans_path(&self) -> PathBuf {
        self.root.join("dca-plans.json")
    }

    /// Alias for [`Self::dca_plans_path`].
    pub fn dca_plans_json(&self) -> PathBuf {
        self.dca_plans_path()
    }

    /// Save policy into the store.
    pub fn save_policy(
        &self,
        policy: &crate::sentient::policy::AgentSessionPolicy,
    ) -> Result<(), AgentError> {
        crate::sentient::policy::save_policy(&self.root, policy)
    }

    /// Load policy from the store.
    pub fn load_policy(&self) -> Result<crate::sentient::policy::AgentSessionPolicy, AgentError> {
        crate::sentient::policy::load_policy(&self.root)
    }

    /// Record a pending stealth destination in the store.
    pub fn record_stealth_pending(
        &self,
        address: alloy::primitives::Address,
    ) -> Result<(), AgentError> {
        crate::sentient::stealth_pending::record_stealth_pending(&self.root, address)
    }

    /// Consume a pending stealth destination from the store.
    pub fn consume_stealth_pending(
        &self,
        address: alloy::primitives::Address,
    ) -> Result<bool, AgentError> {
        crate::sentient::stealth_pending::consume_stealth_pending(&self.root, address)
    }

    /// Add a DCA plan to the store.
    pub fn add_dca_plan(
        &self,
        plan: crate::dca::types::DcaPlan,
    ) -> Result<crate::dca::types::DcaPlan, AgentError> {
        crate::dca::store::add_plan(&self.root, plan)
    }

    /// Cancel a DCA plan in the store.
    pub fn cancel_dca_plan(&self, id: &str) -> Result<crate::dca::types::DcaPlan, AgentError> {
        crate::dca::store::cancel_plan(&self.root, id)
    }

    /// Set paused status of a DCA plan in the store.
    pub fn set_dca_paused(
        &self,
        id: &str,
        paused: bool,
    ) -> Result<crate::dca::types::DcaPlan, AgentError> {
        crate::dca::store::set_paused(&self.root, id, paused)
    }

    /// Load all DCA plans from the store.
    pub fn load_dca_plans(&self) -> Result<Vec<crate::dca::types::DcaPlan>, AgentError> {
        crate::dca::store::load_plans(&self.root).map(|f| f.plans)
    }
}

/// Assert that write operations directed to `target_dir` are valid under current signer mode.
///
/// When signer mode is ON:
/// - Resolves the signer store via [`SignerStore::resolve`].
/// - Fails closed with `signer_store_unavailable` if unconfigured, missing, or wrongly permissioned.
/// - Refuses writes targeting legacy profile directories with `signer_store_unavailable`.
///
/// When signer mode is OFF:
/// - Returns `Ok(())` to preserve existing behavior for pre-S2 operations.
pub fn assert_signer_store_writable(target_dir: &Path) -> Result<(), AgentError> {
    if !is_signer_mode_active() {
        return Ok(());
    }
    let store = SignerStore::resolve(None).map_err(AgentError::from)?;
    match store {
        Some(store) => {
            let store_canon = store
                .root()
                .canonicalize()
                .unwrap_or_else(|_| store.root().to_path_buf());
            let target_canon = target_dir
                .canonicalize()
                .unwrap_or_else(|_| target_dir.to_path_buf());
            if target_canon != store_canon && !target_canon.starts_with(&store_canon) {
                return Err(SignerStoreError::LegacyProfileDirRefused {
                    path: target_dir.to_path_buf(),
                }
                .into());
            }
            Ok(())
        }
        None => Err(SignerStoreError::Unconfigured.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_open_missing_dir_fails_closed() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nonexistent");
        let err = SignerStore::open(&missing).unwrap_err();
        assert!(err.to_string().contains("signer_store_unavailable"));
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn test_open_file_not_dir_fails_closed() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("regular_file");
        fs::write(&file, b"content").unwrap();
        let err = SignerStore::open(&file).unwrap_err();
        assert!(err.to_string().contains("signer_store_unavailable"));
        assert!(err.to_string().contains("not a directory"));
    }

    #[cfg(unix)]
    #[test]
    fn test_open_mode_permissions_and_ownership() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let store_dir = dir.path().join("store");
        fs::create_dir(&store_dir).unwrap();

        // 0700 accepted
        fs::set_permissions(&store_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let store = SignerStore::open(&store_dir).expect("0700 should be accepted");
        assert_eq!(store.root(), store_dir.as_path());

        // 0750 accepted
        fs::set_permissions(&store_dir, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(SignerStore::open(&store_dir).is_ok());

        // 0777 refused
        fs::set_permissions(&store_dir, fs::Permissions::from_mode(0o777)).unwrap();
        let err = SignerStore::open(&store_dir).unwrap_err();
        assert!(err.to_string().contains("signer_store_unavailable"));
        assert!(err.to_string().contains("0777"));
        assert!(err.to_string().contains("chmod 0700"));

        // 0755 refused
        fs::set_permissions(&store_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let err = SignerStore::open(&store_dir).unwrap_err();
        assert!(err.to_string().contains("signer_store_unavailable"));
        assert!(err.to_string().contains("0755"));

        // 0644 refused
        fs::set_permissions(&store_dir, fs::Permissions::from_mode(0o644)).unwrap();
        let err = SignerStore::open(&store_dir).unwrap_err();
        assert!(err.to_string().contains("signer_store_unavailable"));

        // Reset to 0700 and test expected UID mismatch
        fs::set_permissions(&store_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let current_uid = rustix::process::getuid().as_raw();
        let foreign_uid = current_uid + 9999;
        let err = SignerStore::open_with_expected_uid(&store_dir, Some(foreign_uid)).unwrap_err();
        assert!(err.to_string().contains("signer_store_unavailable"));
        assert!(err
            .to_string()
            .contains(&format!("owned by UID {current_uid}")));
        assert!(err
            .to_string()
            .contains(&format!("expected UID {foreign_uid}")));
        assert!(err.to_string().contains(&format!("chown {foreign_uid}")));
    }

    #[test]
    fn test_resolution_precedence() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let store_a = dir.path().join("store_a");
        let store_b = dir.path().join("store_b");
        fs::create_dir(&store_a).unwrap();
        fs::create_dir(&store_b).unwrap();
        #[cfg(unix)]
        {
            fs::set_permissions(&store_a, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(&store_b, fs::Permissions::from_mode(0o700)).unwrap();
        }

        // Neither set, signer mode off -> Ok(None)
        set_signer_mode_override(Some(false));
        let resolved = SignerStore::resolve(None).unwrap();
        assert!(resolved.is_none());

        // Explicit store_root takes precedence over env var
        std::env::set_var(ENV_SIGNER_STORE, store_b.to_str().unwrap());
        let resolved = SignerStore::resolve(Some(&store_a)).unwrap().unwrap();
        assert_eq!(resolved.root(), store_a.as_path());

        // Env var used when explicit is None
        let resolved = SignerStore::resolve(None).unwrap().unwrap();
        assert_eq!(resolved.root(), store_b.as_path());

        // Cleanup env
        std::env::remove_var(ENV_SIGNER_STORE);
        set_signer_mode_override(None);
    }

    #[test]
    fn test_accessors_match_expected_paths() {
        let dir = tempdir().unwrap();
        let store_dir = dir.path().join("store");
        fs::create_dir(&store_dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&store_dir, fs::Permissions::from_mode(0o700)).unwrap();
        }

        let store = SignerStore::open(&store_dir).unwrap();
        assert_eq!(store.root(), store_dir.as_path());
        assert_eq!(store.wallet_path(), store_dir.join("wallet.json"));
        assert_eq!(store.wallet_json(), store_dir.join("wallet.json"));
        assert_eq!(store.policy_path(), store_dir.join("sentient-policy.toml"));
        assert_eq!(
            store.sentient_policy_toml(),
            store_dir.join("sentient-policy.toml")
        );
        assert_eq!(store.spend_path(), store_dir.join("sentient-spend.json"));
        assert_eq!(
            store.sentient_spend_json(),
            store_dir.join("sentient-spend.json")
        );
        assert_eq!(
            store.spend_lock_path(),
            store_dir.join("sentient-spend.lock")
        );
        assert_eq!(
            store.sentient_spend_lock(),
            store_dir.join("sentient-spend.lock")
        );
        assert_eq!(store.proposals_dir(), store_dir.join("proposals"));
        assert_eq!(store.proposals(), store_dir.join("proposals"));
        assert_eq!(
            store.stealth_pending_path(),
            store_dir.join("stealth-pending.json")
        );
        assert_eq!(
            store.stealth_pending_json(),
            store_dir.join("stealth-pending.json")
        );
        assert_eq!(store.dca_plans_path(), store_dir.join("dca-plans.json"));
        assert_eq!(store.dca_plans_json(), store_dir.join("dca-plans.json"));
    }

    #[test]
    fn test_no_fallback_when_signer_mode_is_on() {
        let dir = tempdir().unwrap();
        let old_profile = dir.path().join("old_profile");
        fs::create_dir(&old_profile).unwrap();

        // Signer mode ON, but store is missing: write fails closed with signer_store_unavailable
        set_signer_mode_override(Some(true));
        std::env::set_var(
            ENV_SIGNER_STORE,
            dir.path().join("missing_store").to_str().unwrap(),
        );

        let err = assert_signer_store_writable(&old_profile).unwrap_err();
        assert!(err.to_string().contains("signer_store_unavailable"));

        // Signer mode ON, store unconfigured: write fails closed with signer_store_unavailable
        std::env::remove_var(ENV_SIGNER_STORE);
        let err = assert_signer_store_writable(&old_profile).unwrap_err();
        assert!(err.to_string().contains("signer_store_unavailable"));
        assert!(err.to_string().contains("unconfigured"));

        // Signer mode ON, valid store configured: writes to old profile dir are refused
        let valid_store = dir.path().join("valid_store");
        fs::create_dir(&valid_store).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&valid_store, fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::env::set_var(ENV_SIGNER_STORE, valid_store.to_str().unwrap());

        let err = assert_signer_store_writable(&old_profile).unwrap_err();
        assert!(err.to_string().contains("signer_store_unavailable"));
        assert!(err.to_string().contains("legacy profile directory"));

        // Writes to valid store root succeed
        assert!(assert_signer_store_writable(&valid_store).is_ok());

        // Cleanup
        std::env::remove_var(ENV_SIGNER_STORE);
        set_signer_mode_override(None);

        // Signer mode OFF: old profile directory write check succeeds
        assert!(assert_signer_store_writable(&old_profile).is_ok());
    }
}
