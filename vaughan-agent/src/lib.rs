//! Vaughan Agent: proposal engine, tool registry, and degen circuit breakers.
//!
//! Library-only crate for future MCP integration. Embedded in-wallet LLM chat
//! was retired in 2026-08; this crate exposes structured tools and safety
//! boundaries for external agents.

pub mod degen;
pub mod error;
pub mod paths;
pub mod presets;
pub mod proposal;
pub mod tools;
pub mod types;

pub use degen::{
    breaker_config_for_session, build_policy_proposal, dry_run_from_env, load_policy, save_policy,
    AgentSessionPolicy, CircuitBreaker, CircuitBreakerConfig, DegenTrader, EnforcementMode,
    PolicyProposal, QuorumReserves, QuorumValidator, SwapExecution, DEGEN_POLICY_TOML,
};
pub use error::AgentError;
pub use paths::profile_dir;
pub use presets::{apply_preset, presets_root, BUNDLED_PRESET_IDS};
pub use proposal::{ProposalType, TxProposal};
pub use tools::{
    commit_policy_proposal, default_assist_registry, default_assist_registry_for,
    default_degen_registry, default_sensory_registry, ProposePolicyTool, Tool, ToolContext,
    ToolRegistry,
};
pub use types::ToolDefinition;
