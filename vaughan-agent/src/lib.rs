//! Vaughan Agent: proposal engine, tool registry, and Sentient circuit breakers.
//!
//! Library-only crate for future MCP integration. Embedded in-wallet LLM chat
//! was retired in 2026-08; this crate exposes structured tools and safety
//! boundaries for external agents.

pub mod error;
pub mod paths;
pub mod presets;
pub mod proposal;
pub mod sentient;
pub mod tools;
pub mod types;

pub use error::AgentError;
pub use paths::profile_dir;
pub use presets::{apply_preset, presets_root, BUNDLED_PRESET_IDS};
pub use proposal::{ProposalType, TxProposal};
pub use sentient::{
    breaker_config_for_session, build_policy_proposal, dry_run_from_env, load_policy, save_policy,
    AgentSessionPolicy, CircuitBreaker, CircuitBreakerConfig, EnforcementMode, PolicyProposal,
    QuorumReserves, QuorumValidator, SentientTrader, SwapExecution, LEGACY_DEGEN_POLICY_TOML,
    SENTIENT_POLICY_TOML,
};
pub use tools::{
    commit_policy_proposal, default_assist_registry, default_assist_registry_for,
    default_sensory_registry, default_sentient_registry, ProposePolicyTool, Tool, ToolContext,
    ToolRegistry,
};
pub use types::ToolDefinition;
