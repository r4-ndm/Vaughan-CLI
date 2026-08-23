//! Unified LLM provider layer via [`genai`](https://crates.io/crates/genai).
//!
//! One chat client plugs Ollama, Gemini, OpenAI-compatible gateways (OpenRouter,
//! DeepSeek, Cursor chat proxies, …). Vaughan keeps its own tool registry,
//! TxProposal path, and key vault — `genai` only handles model I/O.
//!
//! Inspired by OpenCode’s provider/model plug UX; implemented with a Rust-native
//! multi-provider client rather than vendoring TypeScript.

mod convert;

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use genai::adapter::AdapterKind;
use genai::chat::{ChatOptions, ChatStreamEvent};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use secrecy::ExposeSecret;
use tokio::sync::{mpsc, watch};

use crate::client::{LlmClient, StreamEvent};
use crate::config::{
    coerce_openrouter_endpoint, is_openrouter_route, normalize_openai_base_url,
    validate_cursor_chat_endpoint, OPENROUTER_BASE_URL,
};
use crate::error::AgentError;
use crate::types::{ChatMessage, ModelConfig, ProviderType, ToolDefinition};

use self::convert::{build_chat_request, chat_message_from_genai_content};

/// Instantiate an [`LlmClient`] from Vaughan [`ModelConfig`] (genai-backed).
pub fn create_llm_client(config: ModelConfig) -> Result<Arc<dyn LlmClient>, AgentError> {
    Ok(Arc::new(GenAiLlmClient::new(config)?))
}

/// Display / routing client wrapping a configured [`genai::Client`].
pub struct GenAiLlmClient {
    display_name: String,
    model_iden: ModelIden,
    client: Client,
    temperature: f32,
}

impl GenAiLlmClient {
    /// Build a genai client pinned to this session’s provider, endpoint, and key.
    pub fn new(mut config: ModelConfig) -> Result<Self, AgentError> {
        if config.provider == ProviderType::Cursor {
            validate_cursor_chat_endpoint(&config.endpoint_url)?;
        }

        // OpenRouter keys (`sk-or-…`) must not hit api.openai.com.
        if matches!(config.provider, ProviderType::OpenAi) {
            let key = config
                .api_key
                .as_ref()
                .map(|k| k.expose_secret().to_string());
            if let Some(url) = coerce_openrouter_endpoint(&config.endpoint_url, key.as_deref()) {
                config.endpoint_url = url;
            }
        }

        let openrouter = matches!(config.provider, ProviderType::OpenAi)
            && is_openrouter_route(
                &config.endpoint_url,
                config.api_key.as_ref().map(|k| k.expose_secret().as_str()),
                &config.model_name,
            );

        let adapter = adapter_kind(config.provider, openrouter);
        let model_name = match config.provider {
            ProviderType::Gemini => crate::types::normalize_gemini_model(&config.model_name),
            _ => config.model_name.clone(),
        };
        let model_iden = ModelIden::new(adapter, model_name.as_str());
        let display_name = if openrouter {
            format!("openrouter/{model_name}")
        } else {
            format!("{}/{}", provider_label(config.provider), model_name)
        };

        let endpoint = endpoint_for(&config, openrouter)?;
        let auth = auth_for(&config)?;
        let temperature = config.temperature;

        let endpoint_for_resolver = endpoint.clone();
        let auth_for_resolver = auth.clone();
        let adapter_for_resolver = adapter;

        let target_resolver = ServiceTargetResolver::from_resolver_fn(
            move |service_target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
                let ServiceTarget { model, .. } = service_target;
                let model = ModelIden::new(adapter_for_resolver, model.model_name);
                Ok(ServiceTarget {
                    endpoint: endpoint_for_resolver.clone(),
                    auth: auth_for_resolver.clone(),
                    model,
                })
            },
        );

        let client = Client::builder()
            .with_service_target_resolver(target_resolver)
            .build();

        Ok(Self {
            display_name,
            model_iden,
            client,
            temperature,
        })
    }

    fn chat_options(&self) -> ChatOptions {
        let mut opts = ChatOptions::default()
            .with_capture_content(true)
            .with_capture_tool_calls(true);
        let model = self.model_iden.model_name.to_string();
        if !model.starts_with("gemini-3") {
            opts = opts.with_temperature(f64::from(self.temperature));
        }
        opts
    }
}

#[async_trait]
impl LlmClient for GenAiLlmClient {
    fn name(&self) -> &str {
        &self.display_name
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<ChatMessage, AgentError> {
        let req = build_chat_request(messages, tools);
        let opts = self.chat_options();
        let resp = self
            .client
            .exec_chat(self.model_iden.clone(), req, Some(&opts))
            .await
            .map_err(map_genai_err)?;
        Ok(chat_message_from_genai_content(resp.content))
    }

    async fn stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        event_tx: mpsc::Sender<StreamEvent>,
        cancel: watch::Receiver<bool>,
    ) -> Result<ChatMessage, AgentError> {
        if *cancel.borrow() {
            return Err(AgentError::ExecutionAborted);
        }

        let req = build_chat_request(messages, tools);
        let opts = self.chat_options();
        let mut stream_resp = self
            .client
            .exec_chat_stream(self.model_iden.clone(), req, Some(&opts))
            .await
            .map_err(map_genai_err)?;

        let mut final_content = None;

        while let Some(item) = stream_resp.stream.next().await {
            if *cancel.borrow() {
                return Err(AgentError::ExecutionAborted);
            }
            match item.map_err(map_genai_err)? {
                ChatStreamEvent::Chunk(chunk) => {
                    if !chunk.content.is_empty() {
                        let _ = event_tx.send(StreamEvent::Delta(chunk.content)).await;
                    }
                }
                ChatStreamEvent::End(end) => {
                    final_content = end.captured_content;
                }
                ChatStreamEvent::Start
                | ChatStreamEvent::ReasoningChunk(_)
                | ChatStreamEvent::ThoughtSignatureChunk(_)
                | ChatStreamEvent::ToolCallChunk(_) => {}
            }
        }

        if *cancel.borrow() {
            return Err(AgentError::ExecutionAborted);
        }

        let content = final_content.unwrap_or_default();
        Ok(chat_message_from_genai_content(content))
    }
}

fn adapter_kind(provider: ProviderType, openrouter: bool) -> AdapterKind {
    match provider {
        ProviderType::Ollama => AdapterKind::Ollama,
        ProviderType::Gemini => AdapterKind::Gemini,
        ProviderType::OpenAi if openrouter => AdapterKind::OpenRouter,
        ProviderType::OpenAi | ProviderType::Cursor => AdapterKind::OpenAI,
    }
}

fn provider_label(provider: ProviderType) -> &'static str {
    match provider {
        ProviderType::Ollama => "ollama",
        ProviderType::Gemini => "gemini",
        ProviderType::OpenAi => "openai",
        ProviderType::Cursor => "cursor",
    }
}

fn endpoint_for(config: &ModelConfig, openrouter: bool) -> Result<Endpoint, AgentError> {
    match config.provider {
        ProviderType::Ollama => {
            let mut url = config.endpoint_url.trim().trim_end_matches('/').to_string();
            if url.is_empty() {
                url = "http://127.0.0.1:11434".into();
            }
            Ok(Endpoint::from_owned(format!("{url}/")))
        }
        ProviderType::Gemini => {
            let url = config.endpoint_url.trim();
            if url.is_empty() || url.contains("generativelanguage.googleapis.com") {
                Ok(Endpoint::from_static(
                    "https://generativelanguage.googleapis.com/v1beta/",
                ))
            } else {
                let base = url.trim_end_matches('/').to_string();
                Ok(Endpoint::from_owned(format!("{base}/")))
            }
        }
        ProviderType::OpenAi | ProviderType::Cursor => {
            let raw = if openrouter && config.endpoint_url.trim().is_empty() {
                OPENROUTER_BASE_URL
            } else {
                config.endpoint_url.as_str()
            };
            let base = normalize_openai_base_url(raw);
            if base.is_empty() {
                return Err(AgentError::ProviderError(
                    "OpenAI-compatible provider requires endpoint_url (or an OpenRouter sk-or-… key)"
                        .into(),
                ));
            }
            Ok(Endpoint::from_owned(format!("{base}/v1/")))
        }
    }
}

fn auth_for(config: &ModelConfig) -> Result<AuthData, AgentError> {
    match config.provider {
        ProviderType::Ollama => Ok(AuthData::from_single("ollama")),
        ProviderType::Gemini | ProviderType::OpenAi | ProviderType::Cursor => {
            let key = config
                .api_key
                .as_ref()
                .map(|k| k.expose_secret().to_string())
                .filter(|k| !k.trim().is_empty())
                .ok_or_else(|| {
                    AgentError::ProviderError(format!(
                        "{} requires an API key",
                        provider_label(config.provider)
                    ))
                })?;
            Ok(AuthData::from_single(key))
        }
    }
}

fn map_genai_err(err: genai::Error) -> AgentError {
    let msg = err.to_string();
    let hint = if msg.contains("401") || msg.to_ascii_lowercase().contains("unauthorized") {
        " Hint: OpenRouter keys (sk-or-…) need https://openrouter.ai/api — Vaughan auto-routes them; re-run /provider if agent.toml still points at api.openai.com. For OpenAI keys use sk-… from platform.openai.com."
    } else {
        ""
    };
    AgentError::ProviderError(format!("{msg}{hint}"))
}
