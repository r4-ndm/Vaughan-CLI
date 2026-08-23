//! Core types for message formats, tool schemas, and model configurations.

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The role of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A structured tool call request emitted by the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    /// Gemini 3.x encrypted reasoning token — must be echoed verbatim on the
    /// next `generateContent` request or the API returns HTTP 400.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// A chat message in a multi-turn conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    pub fn tool_response(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// JSON-schema definition of a tool callable by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Supported LLM provider types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProviderType {
    #[default]
    Ollama,
    Gemini,
    OpenAi,
    /// Cursor API key (`crsr_…`) via an OpenAI-compatible **chat gateway**.
    ///
    /// Cursor's official `api.cursor.com` host does not offer
    /// `POST /v1/chat/completions`. Set `CURSOR_BASE_URL` / `endpoint_url` to a
    /// gateway that speaks that protocol (or use Gemini / OpenAI / Ollama).
    Cursor,
}

/// Default Gemini model id (current GA Flash on Google AI).
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.5-flash";

/// Stronger Gemini option for Assist (Developer API / AI Studio keys).
pub const GEMINI_PRO_MODEL: &str = "gemini-3.5-pro";

/// Map retired / unavailable Gemini Developer API model ids to a current default.
///
/// Note: `gpt-oss-120b` is Vertex AI MaaS only (`gpt-oss-120b-maas`) — it is **not**
/// available with a Gemini API key on `generativelanguage.googleapis.com`.
pub fn normalize_gemini_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.starts_with("gpt-oss-") {
        return DEFAULT_GEMINI_MODEL.to_string();
    }
    match trimmed {
        ""
        | "gemini-1.5-flash"
        | "gemini-1.5-flash-latest"
        | "gemini-1.5-flash-001"
        | "gemini-1.5-flash-002"
        | "gemini-1.5-flash-8b"
        | "gemini-1.5-pro"
        | "gemini-1.5-pro-latest"
        | "gemini-1.5-pro-001"
        | "gemini-1.5-pro-002"
        | "gemini-2.0-flash"
        | "gemini-2.0-flash-001"
        | "gemini-2.0-flash-lite"
        | "gemini-2.0-flash-lite-001"
        | "gemini-2.5-flash"
        | "gemini-2.5-flash-lite"
        | "gemini-2.5-pro"
        | "gemini-3-flash-preview"
        | "gemini-flash-latest" => DEFAULT_GEMINI_MODEL.to_string(),
        other => other.to_string(),
    }
}

/// Model configuration parameters.
#[derive(Clone)]
pub struct ModelConfig {
    pub provider: ProviderType,
    pub endpoint_url: String,
    pub model_name: String,
    pub api_key: Option<SecretString>,
    pub temperature: f32,
}

impl ModelConfig {
    /// Default local Ollama configuration (`http://127.0.0.1:11434`, model `llama3.2`).
    pub fn default_local_ollama() -> Self {
        Self {
            provider: ProviderType::Ollama,
            endpoint_url: "http://127.0.0.1:11434".to_string(),
            model_name: "llama3.2".to_string(),
            api_key: None,
            temperature: 0.2,
        }
    }

    /// Resolve provider settings from environment variables.
    ///
    /// Priority: `CURSOR_API_KEY` → `OPENAI_API_KEY` → `GEMINI_API_KEY` → local Ollama.
    /// Optional overrides: `CURSOR_BASE_URL`, `CURSOR_MODEL`, `OPENAI_BASE_URL`,
    /// `OPENAI_MODEL`, `GEMINI_MODEL`, `OLLAMA_HOST`, `OLLAMA_MODEL`.
    pub fn from_env() -> Self {
        if let Ok(key) = std::env::var("CURSOR_API_KEY") {
            if !key.trim().is_empty() {
                // Empty / missing CURSOR_BASE_URL fails later in create_llm_client
                // with a clear message (api.cursor.com is not a valid chat host).
                let endpoint = std::env::var("CURSOR_BASE_URL").unwrap_or_default();
                let endpoint = endpoint.trim_end_matches('/').trim_end_matches("/v1");
                let model =
                    std::env::var("CURSOR_MODEL").unwrap_or_else(|_| "composer-2".to_string());
                return Self::cursor(endpoint, SecretString::from(key), model);
            }
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            if !key.trim().is_empty() {
                let endpoint = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| {
                    if crate::config::looks_like_openrouter_key(&key) {
                        crate::config::OPENROUTER_BASE_URL.to_string()
                    } else {
                        "https://api.openai.com".to_string()
                    }
                });
                // Strip a trailing `/v1` — genai OpenAI adapter appends `/v1/`.
                let endpoint = endpoint.trim_end_matches('/').trim_end_matches("/v1");
                let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| {
                    if crate::config::looks_like_openrouter_key(&key) {
                        "openrouter/free".to_string()
                    } else {
                        "gpt-4o-mini".to_string()
                    }
                });
                return Self::openai(endpoint, SecretString::from(key), model);
            }
        }
        if let Ok(key) = std::env::var("GEMINI_API_KEY") {
            if !key.trim().is_empty() {
                let model = std::env::var("GEMINI_MODEL")
                    .unwrap_or_else(|_| DEFAULT_GEMINI_MODEL.to_string());
                return Self::gemini(SecretString::from(key), model);
            }
        }

        let mut cfg = Self::default_local_ollama();
        if let Ok(host) = std::env::var("OLLAMA_HOST") {
            if !host.trim().is_empty() {
                cfg.endpoint_url = host;
            }
        }
        if let Ok(model) = std::env::var("OLLAMA_MODEL") {
            if !model.trim().is_empty() {
                cfg.model_name = model;
            }
        }
        cfg
    }

    /// Google Gemini Cloud configuration.
    pub fn gemini(api_key: SecretString, model_name: impl Into<String>) -> Self {
        Self {
            provider: ProviderType::Gemini,
            endpoint_url: "https://generativelanguage.googleapis.com".to_string(),
            model_name: normalize_gemini_model(&model_name.into()),
            api_key: Some(api_key),
            temperature: 0.2,
        }
    }

    /// OpenAI-compatible configuration.
    pub fn openai(
        endpoint_url: impl Into<String>,
        api_key: SecretString,
        model_name: impl Into<String>,
    ) -> Self {
        Self {
            provider: ProviderType::OpenAi,
            endpoint_url: endpoint_url.into(),
            model_name: model_name.into(),
            api_key: Some(api_key),
            temperature: 0.2,
        }
    }

    /// Cursor API key configuration (OpenAI-compatible wire format).
    pub fn cursor(
        endpoint_url: impl Into<String>,
        api_key: SecretString,
        model_name: impl Into<String>,
    ) -> Self {
        Self {
            provider: ProviderType::Cursor,
            endpoint_url: endpoint_url.into(),
            model_name: model_name.into(),
            api_key: Some(api_key),
            temperature: 0.2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_gemini_model, ModelConfig, DEFAULT_GEMINI_MODEL};
    use secrecy::{ExposeSecret, SecretString};

    #[test]
    fn remaps_retired_gemini_models() {
        assert_eq!(
            normalize_gemini_model("gemini-1.5-flash"),
            DEFAULT_GEMINI_MODEL
        );
        assert_eq!(
            normalize_gemini_model("gemini-2.5-flash"),
            DEFAULT_GEMINI_MODEL
        );
        assert_eq!(normalize_gemini_model("gemini-3.5-pro"), "gemini-3.5-pro");
        assert_eq!(normalize_gemini_model("gpt-oss-120b"), DEFAULT_GEMINI_MODEL);
    }

    #[test]
    fn gemini_config_normalizes_retired_default() {
        let cfg = ModelConfig::gemini(
            SecretString::from("test-key".to_string()),
            "gemini-1.5-flash",
        );
        assert_eq!(cfg.model_name, DEFAULT_GEMINI_MODEL);
        assert_eq!(cfg.api_key.as_ref().unwrap().expose_secret(), "test-key");
    }
}
