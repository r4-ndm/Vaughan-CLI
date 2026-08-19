//! LLM Provider implementations.

pub mod gemini;
pub mod ollama;
pub mod openai;

pub use gemini::GeminiClient;
pub use ollama::OllamaClient;
pub use openai::OpenAiClient;

use crate::client::LlmClient;
use crate::error::AgentError;
use crate::types::{ModelConfig, ProviderType};
use std::sync::Arc;

/// Instantiate an [`LlmClient`] based on [`ModelConfig`].
pub fn create_llm_client(config: ModelConfig) -> Result<Arc<dyn LlmClient>, AgentError> {
    match config.provider {
        ProviderType::Ollama => Ok(Arc::new(OllamaClient::new(config))),
        ProviderType::Gemini => Ok(Arc::new(GeminiClient::new(config))),
        ProviderType::OpenAi => Ok(Arc::new(OpenAiClient::new(config))),
    }
}
