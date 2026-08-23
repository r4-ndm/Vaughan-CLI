//! Vaughan Agent: Multi-mode AI subsystem, tool registry, and security sandbox.
//!
//! Provides:
//! - Multi-mode security boundary (Advisor propose-only vs Degen autonomous bot).
//! - LLM provider abstraction ([`client::LlmClient`]) via [`genai`](https://crates.io/crates/genai)
//!   (Ollama, Gemini, OpenAI-compatible, …).
//! - Streaming Assist chat turns ([`chat::run_assist_turn`]).
//! - Mode skills / mandatory rules ([`skills`]) injected into the system prompt.
//! - Structured sensory tools wrapping `wiz4rd-engine` for read-only contract probing.
//! - Proposal engine for drafting human-approved transactions.
//! - Multi-RPC quorum validation and hard circuit breakers for autonomous trading.

pub mod catalog;
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

pub use catalog::{
    models_for_provider, parse_model_ref, parse_provider_id, provider_id, CatalogModel,
};
pub use chat::{
    is_degen_execute_tool, is_propose_tool, is_sensory_tool, run_assist_turn, summarize_tool_json,
    ChatUiEvent, MAX_TOOL_ROUNDS,
};
pub use client::{LlmClient, StreamEvent};
pub use config::{
    clear_api_key, coerce_openrouter_endpoint, is_openrouter_route, load_api_key, load_file_config,
    looks_like_openrouter_key, needs_agent_setup, normalize_openai_base_url, profile_dir,
    resolve_model_config, save_api_key, save_file_config, validate_cursor_chat_endpoint,
    AgentFileConfig, PendingAgentSetup, AGENT_KEY_FILE, AGENT_TOML, CURSOR_NO_CHAT_COMPLETIONS,
    OPENROUTER_BASE_URL,
};
pub use degen::{
    breaker_config_for_session, build_policy_proposal, dry_run_from_env, load_policy, save_policy,
    AgentSessionPolicy, CircuitBreaker, CircuitBreakerConfig, DegenTrader, EnforcementMode,
    PolicyProposal, QuorumReserves, QuorumValidator, SwapExecution, DEGEN_POLICY_TOML,
};
pub use error::AgentError;
pub use proposal::{ProposalType, TxProposal};
pub use providers::create_llm_client;
pub use skills::{
    assist_system_prompt, build_system_prompt, bundled_skills, degen_system_prompt, load_skills,
    skills_for_mode, AgentSessionContext, Skill, SkillKind, SkillMode,
};
pub use tools::{
    commit_policy_proposal, default_assist_registry, default_degen_registry,
    default_sensory_registry, ProposePolicyTool, Tool, ToolContext, ToolRegistry,
};
pub use types::{
    normalize_gemini_model, ChatMessage, ModelConfig, ProviderType, Role, ToolCall, ToolDefinition,
    DEFAULT_GEMINI_MODEL, GEMINI_PRO_MODEL,
};
