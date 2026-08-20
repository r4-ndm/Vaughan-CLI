//! Vaughan Agent: Multi-mode AI subsystem, tool registry, and security sandbox.
//!
//! Provides:
//! - Multi-mode security boundary (Advisor propose-only vs Degen autonomous bot).
//! - LLM provider abstraction ([`client::LlmClient`]) supporting Ollama, Gemini, and OpenAI.
//! - Streaming Assist chat turns ([`chat::run_assist_turn`]).
//! - Mode skills / mandatory rules ([`skills`]) injected into the system prompt.
//! - Structured sensory tools wrapping `wiz4rd-engine` for read-only contract probing.
//! - Proposal engine for drafting human-approved transactions.
//! - Multi-RPC quorum validation and hard circuit breakers for autonomous trading.

pub mod chat;
pub mod client;
pub mod config;
pub mod degen;
pub mod error;
pub mod proposal;
pub mod providers;
pub mod skills;
pub mod tools;
pub mod types;

pub use chat::{run_assist_turn, ChatUiEvent, MAX_TOOL_ROUNDS};
pub use client::{LlmClient, StreamEvent};
pub use config::{
    clear_api_key, load_api_key, load_file_config, needs_agent_setup, profile_dir,
    resolve_model_config, save_api_key, save_file_config, AgentFileConfig, PendingAgentSetup,
    AGENT_KEY_FILE, AGENT_TOML,
};
pub use degen::{
    dry_run_from_env, CircuitBreaker, CircuitBreakerConfig, DegenTrader, QuorumReserves,
    QuorumValidator, SwapExecution,
};
pub use error::AgentError;
pub use proposal::{ProposalType, TxProposal};
pub use providers::create_llm_client;
pub use skills::{
    assist_system_prompt, build_system_prompt, bundled_skills, degen_system_prompt, load_skills,
    skills_for_mode, Skill, SkillKind, SkillMode,
};
pub use tools::{
    default_assist_registry, default_sensory_registry, Tool, ToolContext, ToolRegistry,
};
pub use types::{ChatMessage, ModelConfig, ProviderType, Role, ToolCall, ToolDefinition};
