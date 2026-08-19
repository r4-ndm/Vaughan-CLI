//! Local Ollama provider client.

use crate::client::LlmClient;
use crate::error::AgentError;
use crate::providers::openai::OpenAiClient;
use crate::types::{ChatMessage, ModelConfig, ToolDefinition};
use async_trait::async_trait;

pub struct OllamaClient {
    inner: OpenAiClient,
}

impl OllamaClient {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            inner: OpenAiClient::new(config),
        }
    }
}

#[async_trait]
impl LlmClient for OllamaClient {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<ChatMessage, AgentError> {
        self.inner.complete(messages, tools).await
    }
}
