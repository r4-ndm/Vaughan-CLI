//! LLM client abstraction.

use crate::error::AgentError;
use crate::types::{ChatMessage, ToolDefinition};
use async_trait::async_trait;
use tokio::sync::{mpsc, watch};

/// Incremental events emitted while an LLM completion is streaming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// Fresh assistant text (append to the live reply).
    Delta(String),
}

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

    /// Stream a chat completion, forwarding text deltas on `event_tx`.
    ///
    /// Default implementation calls [`Self::complete`] and emits a single
    /// delta when the provider has no native stream (e.g. Gemini fallback).
    /// Set `*cancel` to `true` to abort; the method returns
    /// [`AgentError::ExecutionAborted`].
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
        let message = self.complete(messages, tools).await?;
        if *cancel.borrow() {
            return Err(AgentError::ExecutionAborted);
        }
        if !message.content.is_empty() {
            let _ = event_tx
                .send(StreamEvent::Delta(message.content.clone()))
                .await;
        }
        Ok(message)
    }
}
