//! Persistent agent provider settings (`agent.toml`) and encrypted API keys.
//!
//! Non-secret fields (provider, model, endpoint) live in plaintext TOML next to
//! the vault. API keys are encrypted with the vault password (Argon2id +
//! AES-256-GCM) into `agent.key.json` — never written in the clear.

use std::fs;
use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use vaughan_core::security::encryption::{decrypt, encrypt, EncryptedVault};

use crate::error::AgentError;
use crate::types::{ModelConfig, ProviderType};

/// Filename for non-secret agent settings (beside `wallet.json`).
pub const AGENT_TOML: &str = "agent.toml";
/// Filename for the encrypted API key blob.
pub const AGENT_KEY_FILE: &str = "agent.key.json";

/// Default OpenRouter OpenAI-compatible base (no trailing `/v1` — clients add it).
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api";

/// OpenRouter keys look like `sk-or-v1-…` (not OpenAI `sk-…`).
pub fn looks_like_openrouter_key(key: &str) -> bool {
    key.trim().starts_with("sk-or-")
}

/// True when this OpenAI-compatible session should hit OpenRouter, not api.openai.com.
pub fn is_openrouter_route(endpoint_url: &str, api_key: Option<&str>, model_name: &str) -> bool {
    let ep = endpoint_url.to_ascii_lowercase();
    if ep.contains("openrouter.ai") {
        return true;
    }
    if model_name
        .trim()
        .to_ascii_lowercase()
        .starts_with("openrouter/")
    {
        return true;
    }
    api_key.map(looks_like_openrouter_key).unwrap_or(false)
}

/// If an OpenRouter key was pointed at OpenAI’s host (or no host), rewrite to OpenRouter.
pub fn coerce_openrouter_endpoint(endpoint_url: &str, api_key: Option<&str>) -> Option<String> {
    let key = api_key?;
    if !looks_like_openrouter_key(key) {
        return None;
    }
    let ep = endpoint_url.trim().to_ascii_lowercase();
    if ep.is_empty() || ep.contains("api.openai.com") {
        return Some(OPENROUTER_BASE_URL.to_string());
    }
    None
}

/// Why Cursor dashboard keys cannot drive Vaughan Assist chat directly.
pub const CURSOR_NO_CHAT_COMPLETIONS: &str = "Cursor dashboard API keys do not expose OpenAI chat completions (POST /v1/chat/completions returns 404 on api.cursor.com). Set CURSOR_BASE_URL / endpoint_url to an OpenAI-compatible gateway that speaks chat completions, or use Gemini, OpenAI, or Ollama for Vaughan Assist.";

/// Strip trailing `/` and optional `/v1` (clients append `/v1/chat/completions`).
pub fn normalize_openai_base_url(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_string()
}

/// Cursor Assist chat needs an OpenAI-compatible gateway — not `api.cursor.com`.
pub fn validate_cursor_chat_endpoint(url: &str) -> Result<String, AgentError> {
    let endpoint = normalize_openai_base_url(url);
    if endpoint.is_empty() {
        return Err(AgentError::ProviderError(
            "Cursor requires CURSOR_BASE_URL (or endpoint_url in agent.toml) pointing at an OpenAI-compatible chat gateway. Cursor's official API is Cloud Agents / SDK only — not chat completions. Prefer Gemini, OpenAI, or Ollama for Vaughan Assist."
                .into(),
        ));
    }
    if endpoint.to_ascii_lowercase().contains("api.cursor.com") {
        return Err(AgentError::ProviderError(CURSOR_NO_CHAT_COMPLETIONS.into()));
    }
    Ok(endpoint)
}

/// On-disk agent settings (no secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFileConfig {
    /// `"ollama"`, `"gemini"`, `"openai"`, or `"cursor"`.
    pub provider: String,
    /// Model id (e.g. `llama3.2`, `gemini-3.5-flash`, `gpt-4o-mini`, `composer-2`).
    pub model: String,
    /// Optional custom base URL (OpenAI-compatible gateways / Ollama host / Cursor).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
}

impl AgentFileConfig {
    pub fn ollama(model: impl Into<String>) -> Self {
        Self {
            provider: "ollama".into(),
            model: model.into(),
            endpoint_url: None,
        }
    }

    pub fn gemini(model: impl Into<String>) -> Self {
        Self {
            provider: "gemini".into(),
            model: model.into(),
            endpoint_url: None,
        }
    }

    pub fn openai(model: impl Into<String>, endpoint_url: Option<String>) -> Self {
        Self {
            provider: "openai".into(),
            model: model.into(),
            endpoint_url,
        }
    }

    /// Cursor key + OpenAI-compatible chat gateway (`endpoint_url` / `CURSOR_BASE_URL`).
    ///
    /// Does **not** default to `api.cursor.com` — that host has no chat completions.
    pub fn cursor(model: impl Into<String>) -> Self {
        Self {
            provider: "cursor".into(),
            model: model.into(),
            endpoint_url: None,
        }
    }

    pub fn provider_type(&self) -> Result<ProviderType, AgentError> {
        match self.provider.to_ascii_lowercase().as_str() {
            "ollama" => Ok(ProviderType::Ollama),
            "gemini" => Ok(ProviderType::Gemini),
            "openai" => Ok(ProviderType::OpenAi),
            "cursor" => Ok(ProviderType::Cursor),
            other => Err(AgentError::ProviderError(format!(
                "unknown agent provider in {AGENT_TOML}: {other}"
            ))),
        }
    }

    /// Build a [`ModelConfig`], attaching `api_key` when the provider needs one.
    pub fn to_model_config(
        &self,
        api_key: Option<SecretString>,
    ) -> Result<ModelConfig, AgentError> {
        let provider = self.provider_type()?;
        let mut cfg = match provider {
            ProviderType::Ollama => {
                let mut c = ModelConfig::default_local_ollama();
                c.model_name = self.model.clone();
                if let Some(ref url) = self.endpoint_url {
                    c.endpoint_url = url.clone();
                }
                c
            }
            ProviderType::Gemini => {
                let key = api_key.ok_or_else(|| {
                    AgentError::ProviderError(
                        "Gemini requires an API key — re-run agent setup or set GEMINI_API_KEY"
                            .into(),
                    )
                })?;
                ModelConfig::gemini(key, self.model.clone())
            }
            ProviderType::OpenAi => {
                let key = api_key.ok_or_else(|| {
                    AgentError::ProviderError(
                        "OpenAI-compatible providers require an API key — re-run agent setup or set OPENAI_API_KEY"
                            .into(),
                    )
                })?;
                let mut endpoint = self
                    .endpoint_url
                    .clone()
                    .unwrap_or_else(|| "https://api.openai.com".into());
                if let Some(or_url) =
                    coerce_openrouter_endpoint(&endpoint, Some(key.expose_secret()))
                {
                    endpoint = or_url;
                }
                ModelConfig::openai(endpoint, key, self.model.clone())
            }
            ProviderType::Cursor => {
                let key = api_key.ok_or_else(|| {
                    AgentError::ProviderError(
                        "Cursor requires an API key — re-run agent setup or set CURSOR_API_KEY"
                            .into(),
                    )
                })?;
                let endpoint =
                    validate_cursor_chat_endpoint(self.endpoint_url.as_deref().unwrap_or(""))?;
                ModelConfig::cursor(endpoint, key, self.model.clone())
            }
        };
        cfg.provider = provider;
        Ok(cfg)
    }
}

/// Directory that contains `wallet.json` / `agent.toml` for a profile.
pub fn profile_dir(wallet_path: &Path) -> PathBuf {
    wallet_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Load `agent.toml` if present.
pub fn load_file_config(dir: &Path) -> Result<Option<AgentFileConfig>, AgentError> {
    let path = dir.join(AGENT_TOML);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| {
        AgentError::ProviderError(format!("failed to read {}: {e}", path.display()))
    })?;
    let cfg: AgentFileConfig = toml::from_str(&raw)
        .map_err(|e| AgentError::ProviderError(format!("invalid {AGENT_TOML}: {e}")))?;
    Ok(Some(cfg))
}

/// Atomically write `agent.toml`.
pub fn save_file_config(dir: &Path, cfg: &AgentFileConfig) -> Result<(), AgentError> {
    fs::create_dir_all(dir).map_err(|e| {
        AgentError::ProviderError(format!("failed to create {}: {e}", dir.display()))
    })?;
    let path = dir.join(AGENT_TOML);
    let raw = toml::to_string_pretty(cfg)
        .map_err(|e| AgentError::ProviderError(format!("serialize {AGENT_TOML}: {e}")))?;
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, raw)
        .map_err(|e| AgentError::ProviderError(format!("write {}: {e}", tmp.display())))?;
    fs::rename(&tmp, &path)
        .map_err(|e| AgentError::ProviderError(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

/// Encrypt and persist an API key under the vault password.
pub fn save_api_key(
    dir: &Path,
    password: &SecretString,
    api_key: &SecretString,
) -> Result<(), AgentError> {
    fs::create_dir_all(dir).map_err(|e| {
        AgentError::ProviderError(format!("failed to create {}: {e}", dir.display()))
    })?;
    let vault = encrypt(api_key.expose_secret().as_bytes(), password)
        .map_err(|e| AgentError::ProviderError(format!("encrypt API key failed: {e}")))?;
    let path = dir.join(AGENT_KEY_FILE);
    let raw = serde_json::to_string_pretty(&vault)
        .map_err(|e| AgentError::ProviderError(format!("serialize {AGENT_KEY_FILE}: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw)
        .map_err(|e| AgentError::ProviderError(format!("write {}: {e}", tmp.display())))?;
    fs::rename(&tmp, &path)
        .map_err(|e| AgentError::ProviderError(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

/// Decrypt a previously saved API key, if the file exists.
pub fn load_api_key(
    dir: &Path,
    password: &SecretString,
) -> Result<Option<SecretString>, AgentError> {
    let path = dir.join(AGENT_KEY_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| {
        AgentError::ProviderError(format!("failed to read {}: {e}", path.display()))
    })?;
    let vault: EncryptedVault = serde_json::from_str(&raw)
        .map_err(|e| AgentError::ProviderError(format!("invalid {AGENT_KEY_FILE}: {e}")))?;
    let bytes = decrypt(&vault, password)
        .map_err(|e| AgentError::ProviderError(format!("decrypt API key failed: {e}")))?;
    let key = String::from_utf8(bytes)
        .map_err(|_| AgentError::ProviderError("API key is not valid UTF-8".into()))?;
    Ok(Some(SecretString::from(key)))
}

/// Remove a saved encrypted API key (e.g. switching to Ollama).
pub fn clear_api_key(dir: &Path) -> Result<(), AgentError> {
    let path = dir.join(AGENT_KEY_FILE);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| {
            AgentError::ProviderError(format!("failed to remove {}: {e}", path.display()))
        })?;
    }
    Ok(())
}

/// Resolve the active [`ModelConfig`]: file + encrypted key, else env defaults.
pub fn resolve_model_config(
    dir: &Path,
    password: Option<&SecretString>,
) -> Result<ModelConfig, AgentError> {
    if let Some(file) = load_file_config(dir)? {
        let key = match password {
            Some(pw) => load_api_key(dir, pw)?,
            None => None,
        };
        // Cloud providers without a decrypted key still try env as a fallback.
        let key = match key {
            Some(k) => Some(k),
            None => match file.provider_type() {
                Ok(p) => env_key_for_provider(p),
                Err(_) => None,
            },
        };
        return file.to_model_config(key);
    }
    Ok(ModelConfig::from_env())
}

/// True when AI mode should prompt for provider/API key setup.
///
/// Triggers when:
/// - no `agent.toml` yet (first AI session on this profile), or
/// - configured Gemini/OpenAI/Cursor but neither an encrypted key nor env key is available.
///
/// When an encrypted `agent.key.json` exists for the vault password, setup is
/// skipped so the next login reuses the saved key without re-entry.
pub fn needs_agent_setup(dir: &Path, password: Option<&SecretString>) -> bool {
    match load_file_config(dir) {
        Ok(None) => true,
        Ok(Some(file)) => match file.provider_type() {
            Ok(ProviderType::Ollama) => false,
            Ok(provider) => {
                let has_file_key = password
                    .and_then(|pw| load_api_key(dir, pw).ok().flatten())
                    .is_some();
                let has_env = env_key_for_provider(provider).is_some();
                !(has_file_key || has_env)
            }
            Err(_) => true,
        },
        Err(_) => true,
    }
}

fn env_key_for_provider(provider: ProviderType) -> Option<SecretString> {
    let var = match provider {
        ProviderType::Gemini => "GEMINI_API_KEY",
        ProviderType::OpenAi => "OPENAI_API_KEY",
        ProviderType::Cursor => "CURSOR_API_KEY",
        ProviderType::Ollama => return None,
    };
    std::env::var(var)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(SecretString::from)
}

/// In-memory setup collected on the welcome screen before the vault exists.
#[derive(Clone)]
pub struct PendingAgentSetup {
    pub file: AgentFileConfig,
    pub api_key: Option<SecretString>,
}

impl PendingAgentSetup {
    /// Persist toml (+ encrypted key when password + key are present).
    pub fn persist(&self, dir: &Path, password: Option<&SecretString>) -> Result<(), AgentError> {
        save_file_config(dir, &self.file)?;
        match (&self.api_key, password) {
            (Some(key), Some(pw)) => save_api_key(dir, pw, key)?,
            (None, _) => clear_api_key(dir)?,
            (Some(_), None) => {
                // Key stays session-only until a password is available.
            }
        }
        Ok(())
    }

    pub fn to_model_config(&self) -> Result<ModelConfig, AgentError> {
        self.file.to_model_config(self.api_key.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn strong_pw() -> SecretString {
        SecretString::from("CorrectHorse9!BatteryStaple".to_string())
    }

    #[test]
    fn openrouter_key_coerces_openai_host() {
        assert!(looks_like_openrouter_key("sk-or-v1-abc"));
        assert!(!looks_like_openrouter_key("sk-proj-abc"));
        assert_eq!(
            coerce_openrouter_endpoint("https://api.openai.com", Some("sk-or-v1-abc")),
            Some(OPENROUTER_BASE_URL.to_string())
        );
        assert!(
            coerce_openrouter_endpoint("https://openrouter.ai/api", Some("sk-or-v1-abc")).is_none()
        );
        assert!(coerce_openrouter_endpoint("https://api.openai.com", Some("sk-proj-x")).is_none());
    }

    #[test]
    fn openai_toml_with_openrouter_key_rewrites_endpoint() {
        let dir = tempdir().unwrap();
        let pw = strong_pw();
        // Misconfigured: OpenRouter key but openai.com host from an older setup.
        save_file_config(
            dir.path(),
            &AgentFileConfig::openai("openrouter/free", Some("https://api.openai.com".into())),
        )
        .unwrap();
        save_api_key(
            dir.path(),
            &pw,
            &SecretString::from("sk-or-v1-testkey".to_string()),
        )
        .unwrap();
        let cfg = resolve_model_config(dir.path(), Some(&pw)).unwrap();
        assert!(cfg.endpoint_url.contains("openrouter.ai"));
    }

    #[test]
    fn api_key_encrypt_roundtrip() {
        let dir = tempdir().unwrap();
        let pw = strong_pw();
        let key = SecretString::from("sk-test-secret".to_string());
        save_api_key(dir.path(), &pw, &key).unwrap();
        let loaded = load_api_key(dir.path(), &pw).unwrap().unwrap();
        assert_eq!(loaded.expose_secret(), "sk-test-secret");
    }

    #[test]
    fn pending_ollama_persists_without_key_file() {
        let dir = tempdir().unwrap();
        let pending = PendingAgentSetup {
            file: AgentFileConfig::ollama("llama3.2"),
            api_key: None,
        };
        pending.persist(dir.path(), Some(&strong_pw())).unwrap();
        assert!(dir.path().join(AGENT_TOML).exists());
        assert!(!dir.path().join(AGENT_KEY_FILE).exists());
        let cfg = resolve_model_config(dir.path(), Some(&strong_pw())).unwrap();
        assert_eq!(cfg.provider, ProviderType::Ollama);
        assert_eq!(cfg.model_name, "llama3.2");
    }

    #[test]
    fn needs_setup_when_no_toml() {
        let dir = tempdir().unwrap();
        assert!(needs_agent_setup(dir.path(), None));
    }

    #[test]
    fn ollama_toml_does_not_need_setup() {
        let dir = tempdir().unwrap();
        save_file_config(dir.path(), &AgentFileConfig::ollama("llama3.2")).unwrap();
        assert!(!needs_agent_setup(dir.path(), None));
    }

    #[test]
    fn openai_toml_with_encrypted_key_skips_setup() {
        let dir = tempdir().unwrap();
        let pw = strong_pw();
        save_file_config(
            dir.path(),
            &AgentFileConfig::openai("openrouter/free", Some("https://openrouter.ai/api".into())),
        )
        .unwrap();
        save_api_key(
            dir.path(),
            &pw,
            &SecretString::from("sk-or-v1-persisted".to_string()),
        )
        .unwrap();
        assert!(!needs_agent_setup(dir.path(), Some(&pw)));
        let cfg = resolve_model_config(dir.path(), Some(&pw)).unwrap();
        assert_eq!(
            cfg.api_key.as_ref().unwrap().expose_secret(),
            "sk-or-v1-persisted"
        );
    }

    #[test]
    fn openai_toml_needs_setup_without_key() {
        let dir = tempdir().unwrap();
        save_file_config(dir.path(), &AgentFileConfig::openai("gpt-4o-mini", None)).unwrap();
        assert!(needs_agent_setup(dir.path(), Some(&strong_pw())));
    }

    #[test]
    fn cursor_rejects_official_api_host() {
        let err = validate_cursor_chat_endpoint("https://api.cursor.com")
            .expect_err("api.cursor.com must fail");
        let msg = err.to_string();
        assert!(msg.contains("chat completions") || msg.contains("api.cursor.com"));
    }

    #[test]
    fn cursor_accepts_local_gateway() {
        let url = validate_cursor_chat_endpoint("http://127.0.0.1:8765/v1").unwrap();
        assert_eq!(url, "http://127.0.0.1:8765");
    }
}
