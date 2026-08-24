//! Bundled sentient skill + policy presets (`high-risk-gambler`, …).

use std::fs;
use std::path::{Path, PathBuf};

use crate::degen::policy::DEGEN_POLICY_TOML;
use crate::error::AgentError;

/// Ids shipped under `vaughan-agent/presets/`.
pub const BUNDLED_PRESET_IDS: &[&str] = &[
    "high-risk-gambler",
    "balanced",
    "quant-risk-reward",
    "cautious",
];

/// Directory containing preset folders (override with `VAUGHAN_PRESETS_DIR`).
pub fn presets_root() -> PathBuf {
    if let Ok(p) = std::env::var("VAUGHAN_PRESETS_DIR") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("presets")
}

/// Copy a bundled preset’s `SKILL.md` + `policy.toml` into a profile directory.
pub fn apply_preset(preset_id: &str, profile_dir: &Path) -> Result<PathBuf, AgentError> {
    if !BUNDLED_PRESET_IDS.contains(&preset_id) {
        return Err(AgentError::InvalidToolCall(format!(
            "unknown preset `{preset_id}` — try: {}",
            BUNDLED_PRESET_IDS.join(", ")
        )));
    }
    let src = presets_root().join(preset_id);
    if !src.is_dir() {
        return Err(AgentError::ProviderError(format!(
            "preset files missing at {} (set VAUGHAN_PRESETS_DIR?)",
            src.display()
        )));
    }

    let skill_src = src.join("SKILL.md");
    let policy_src = src.join("policy.toml");
    if !skill_src.is_file() || !policy_src.is_file() {
        return Err(AgentError::ProviderError(format!(
            "preset `{preset_id}` needs SKILL.md and policy.toml"
        )));
    }

    let skill_dst_dir = profile_dir.join("skills").join(preset_id);
    fs::create_dir_all(&skill_dst_dir)
        .map_err(|e| AgentError::ProviderError(format!("create skills dir: {e}")))?;
    let skill_dst = skill_dst_dir.join("SKILL.md");
    fs::copy(&skill_src, &skill_dst)
        .map_err(|e| AgentError::ProviderError(format!("copy SKILL.md: {e}")))?;

    let policy_dst = profile_dir.join(DEGEN_POLICY_TOML);
    fs::copy(&policy_src, &policy_dst)
        .map_err(|e| AgentError::ProviderError(format!("copy policy: {e}")))?;

    Ok(skill_dst_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn apply_balanced_copies_skill_and_policy() {
        let dir = tempdir().unwrap();
        apply_preset("balanced", dir.path()).unwrap();
        assert!(dir.path().join("skills/balanced/SKILL.md").is_file());
        assert!(dir.path().join(DEGEN_POLICY_TOML).is_file());
    }
}
