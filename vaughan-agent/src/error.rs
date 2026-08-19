//! Agent error types.

use thiserror::Error;

/// Errors arising from the AI agent subsystem.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("LLM Provider error: {0}")]
    ProviderError(String),

    #[error("Network / HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Invalid tool call: {0}")]
    InvalidToolCall(String),

    #[error("Circuit breaker tripped: {0}")]
    CircuitBreakerTripped(String),

    #[error("Multi-RPC Quorum mismatch: primary and secondary RPCs differ by > 0.5%")]
    RpcQuorumMismatch,

    #[error("Agent execution aborted by user or kill switch")]
    ExecutionAborted,

    #[error("Operation not permitted in current operating mode: {0}")]
    ModeViolation(String),

    #[error("Security violation: {0}")]
    SecurityViolation(String),
}
