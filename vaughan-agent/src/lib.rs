//! Vaughan Agent: Multi-mode AI subsystem, tool registry, and security sandbox.
//!
//! Provides:
//! - Multi-mode security boundary (Advisor propose-only vs Degen autonomous bot).
//! - LLM provider abstraction ([`client::LlmClient`]) supporting Ollama, Gemini, and OpenAI.
//! - Structured sensory tools wrapping `wiz4rd-engine` for read-only contract probing.
//! - Proposal engine for drafting human-approved transactions.
//! - Multi-RPC quorum validation and hard circuit breakers for autonomous trading.

pub mod client;
pub mod error;
pub mod providers;
pub mod types;

pub use client::LlmClient;
pub use error::AgentError;
pub use providers::create_llm_client;
pub use types::{ChatMessage, ModelConfig, ProviderType, Role, ToolCall, ToolDefinition};
