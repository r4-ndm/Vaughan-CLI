//! On-disk Piteas settings (`piteas.toml`) and encrypted partner API key.

use std::fs;
use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::error::WalletError;
use crate::security::encryption::{decrypt, encrypt, EncryptedVault};

/// Non-secret settings beside `wallet.json`.
pub const PITEAS_TOML: &str = "piteas.toml";
/// Encrypted partner API key blob (Argon2id + AES-256-GCM).
pub const PITEAS_KEY_FILE: &str = "piteas.key.json";

/// How to attach a partner API key when one is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AuthStyle {
    /// Public SDK beta — no auth header (default today).
    #[default]
    None,
    /// `Authorization: Bearer <key>`
    Bearer,
    /// `X-API-Key: <key>`
    #[serde(rename = "x-api-key")]
    XApiKey,
    /// `?apiKey=<key>` query parameter
    Query,
}

impl AuthStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bearer => "bearer",
            Self::XApiKey => "x-api-key",
            Self::Query => "query",
        }
    }
}

impl std::str::FromStr for AuthStyle {
    type Err = WalletError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "" => Ok(Self::None),
            "bearer" => Ok(Self::Bearer),
            "x-api-key" | "x_api_key" | "apikey" | "api-key" => Ok(Self::XApiKey),
            "query" | "query-param" => Ok(Self::Query),
            other => Err(WalletError::Other(format!(
                "unknown piteas auth_style '{other}' (use none|bearer|x-api-key|query)"
            ))),
        }
    }
}

/// Plaintext partner / SDK settings (no secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiteasFileConfig {
    /// Quote API origin (no trailing slash). Default: `https://sdk.piteas.io`.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// How to send [`crate::core::piteas::load_api_key`] when present.
    #[serde(default)]
    pub auth_style: AuthStyle,
    /// Soft client-side rate hint (requests per minute). Beta docs say 10.
    #[serde(default = "default_rpm")]
    pub max_requests_per_minute: u32,
}

fn default_base_url() -> String {
    "https://sdk.piteas.io".into()
}

fn default_rpm() -> u32 {
    10
}

impl Default for PiteasFileConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            auth_style: AuthStyle::None,
            max_requests_per_minute: default_rpm(),
        }
    }
}

impl PiteasFileConfig {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(PITEAS_TOML)
    }
}

/// Load `piteas.toml` if present.
pub fn load_file_config(dir: &Path) -> Result<Option<PiteasFileConfig>, WalletError> {
    let path = PiteasFileConfig::path(dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| WalletError::Io(format!("read {}: {e}", path.display())))?;
    let cfg: PiteasFileConfig = toml_from_str(&raw)?;
    Ok(Some(cfg))
}

/// Persist non-secret Piteas settings (atomic write).
pub fn save_file_config(dir: &Path, cfg: &PiteasFileConfig) -> Result<(), WalletError> {
    fs::create_dir_all(dir)
        .map_err(|e| WalletError::Io(format!("create {}: {e}", dir.display())))?;
    let path = PiteasFileConfig::path(dir);
    let raw = toml_to_string(cfg)?;
    atomic_write(&path, raw.as_bytes())?;
    Ok(())
}

/// Encrypt and store a partner API key under the vault password.
pub fn save_api_key(
    dir: &Path,
    password: &SecretString,
    api_key: &SecretString,
) -> Result<(), WalletError> {
    fs::create_dir_all(dir)
        .map_err(|e| WalletError::Io(format!("create {}: {e}", dir.display())))?;
    let vault = encrypt(api_key.expose_secret().as_bytes(), password)?;
    let path = dir.join(PITEAS_KEY_FILE);
    let raw = serde_json::to_string_pretty(&vault)
        .map_err(|e| WalletError::Serialization(format!("serialize {PITEAS_KEY_FILE}: {e}")))?;
    atomic_write(&path, raw.as_bytes())?;
    Ok(())
}

/// Decrypt a saved partner key, if the blob exists.
pub fn load_api_key(
    dir: &Path,
    password: &SecretString,
) -> Result<Option<SecretString>, WalletError> {
    let path = dir.join(PITEAS_KEY_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| WalletError::Io(format!("read {}: {e}", path.display())))?;
    let vault: EncryptedVault = serde_json::from_str(&raw)
        .map_err(|e| WalletError::Serialization(format!("invalid {PITEAS_KEY_FILE}: {e}")))?;
    let bytes = decrypt(&vault, password)?;
    let key = String::from_utf8(bytes)
        .map_err(|_| WalletError::DecryptionFailed("piteas API key is not valid UTF-8".into()))?;
    Ok(Some(SecretString::from(key)))
}

/// Remove a saved encrypted partner key.
pub fn clear_api_key(dir: &Path) -> Result<(), WalletError> {
    let path = dir.join(PITEAS_KEY_FILE);
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|e| WalletError::Io(format!("remove {}: {e}", path.display())))?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), WalletError> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|e| WalletError::Io(format!("write {}: {e}", tmp.display())))?;
    fs::rename(&tmp, path)
        .map_err(|e| WalletError::Io(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

/// Minimal TOML parse without adding a workspace dep — only our flat struct.
fn toml_from_str(raw: &str) -> Result<PiteasFileConfig, WalletError> {
    // Prefer serde via `toml` if present; otherwise hand-parse known keys.
    // Workspace does not list `toml` yet — hand-parse to stay on the allowlist.
    let mut cfg = PiteasFileConfig::default();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches('"').trim_matches('\'');
        match k {
            "base_url" => cfg.base_url = v.to_string(),
            "auth_style" => cfg.auth_style = v.parse()?,
            "max_requests_per_minute" => {
                cfg.max_requests_per_minute = v.parse().map_err(|_| {
                    WalletError::Other(format!("max_requests_per_minute is not a number: {v}"))
                })?;
            }
            _ => {}
        }
    }
    Ok(cfg)
}

fn toml_to_string(cfg: &PiteasFileConfig) -> Result<String, WalletError> {
    Ok(format!(
        "# Vaughan Piteas aggregator settings (no secrets)\n\
         # Partner API key lives in {PITEAS_KEY_FILE} (encrypted).\n\
         base_url = \"{}\"\n\
         auth_style = \"{}\"\n\
         max_requests_per_minute = {}\n",
        cfg.base_url,
        cfg.auth_style.as_str(),
        cfg.max_requests_per_minute
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn file_config_roundtrip() {
        let dir = TempDir::new().unwrap();
        let cfg = PiteasFileConfig {
            base_url: "https://partner.example/sdk".into(),
            auth_style: AuthStyle::Bearer,
            max_requests_per_minute: 30,
        };
        save_file_config(dir.path(), &cfg).unwrap();
        let loaded = load_file_config(dir.path()).unwrap().unwrap();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn api_key_encrypt_roundtrip() {
        let dir = TempDir::new().unwrap();
        let pw = SecretString::from("CorrectHorse9!BatteryStaple".to_string());
        let key = SecretString::from("piteas-partner-key-xyz".to_string());
        save_api_key(dir.path(), &pw, &key).unwrap();
        let loaded = load_api_key(dir.path(), &pw).unwrap().unwrap();
        assert_eq!(loaded.expose_secret(), key.expose_secret());
        clear_api_key(dir.path()).unwrap();
        assert!(load_api_key(dir.path(), &pw).unwrap().is_none());
    }
}
