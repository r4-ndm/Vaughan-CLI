//! LLM client abstraction.

use crate::error::AgentError;
use crate::types::{ChatMessage, ToolDefinition};
use async_trait::async_trait;

/// Trait for communicating with LLM inference engines (local or cloud).
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Human-readable name of the provider / model.
    fn name(&self) -> &str;

    /// Execute a chat completion with optional tool declarations.
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<ChatMessage, AgentError>;
}
