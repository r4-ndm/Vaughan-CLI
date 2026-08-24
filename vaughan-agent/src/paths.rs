//! Profile directory helpers for agent-side files (`degen-policy.toml`, etc.).

use std::path::{Path, PathBuf};

/// Directory that contains `wallet.json` for a profile.
pub fn profile_dir(wallet_path: &Path) -> PathBuf {
    wallet_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
