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
    /// Priority: `OPENAI_API_KEY` → `GEMINI_API_KEY` → local Ollama.
    /// Optional overrides: `OPENAI_BASE_URL`, `OPENAI_MODEL`, `GEMINI_MODEL`,
    /// `OLLAMA_HOST`, `OLLAMA_MODEL`.
    pub fn from_env() -> Self {
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            if !key.trim().is_empty() {
                let endpoint = std::env::var("OPENAI_BASE_URL")
                    .unwrap_or_else(|_| "https://api.openai.com".to_string());
                // Strip a trailing `/v1` — OpenAiClient appends `/v1/chat/completions`.
                let endpoint = endpoint.trim_end_matches('/').trim_end_matches("/v1");
                let model =
                    std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
                return Self::openai(endpoint, SecretString::from(key), model);
            }
        }
        if let Ok(key) = std::env::var("GEMINI_API_KEY") {
            if !key.trim().is_empty() {
                let model = std::env::var("GEMINI_MODEL")
                    .unwrap_or_else(|_| "gemini-1.5-flash".to_string());
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
            model_name: model_name.into(),
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
}
