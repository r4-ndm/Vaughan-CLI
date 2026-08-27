//! Sentient Mode autonomous trading engine and risk boundaries.

pub mod circuit_breaker;
pub mod policy;
pub mod quorum;
pub mod trader;

pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
pub use policy::{
    breaker_config_for_session, build_policy_proposal, load_policy, save_policy,
    AgentSessionPolicy, EnforcementMode, PolicyProposal, LEGACY_DEGEN_POLICY_TOML,
    SENTIENT_POLICY_TOML,
};
pub use quorum::{QuorumReserves, QuorumValidator};
pub use trader::{dry_run_from_env, SentientTrader, SwapExecution};
