//! Sentient Mode autonomous trading engine and risk boundaries.

pub mod circuit_breaker;
pub mod file_guard;
pub mod policy;
pub mod quorum;
pub mod signer_key;
pub mod spend_ledger;
pub mod stealth_pending;
pub mod trader;

pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
pub use file_guard::assert_owner_only_file;
pub use policy::{
    breaker_config_for_session, build_policy_proposal, load_policy, save_policy,
    AgentSessionPolicy, EnforcementMode, PolicyProposal, LEGACY_DEGEN_POLICY_TOML,
    SENTIENT_POLICY_TOML,
};
pub use quorum::{QuorumReserves, QuorumValidator};
pub use signer_key::{
    derive_sentient_queue_key, init_signer_key, load_signer_key, sentient_queue_hmac_key,
    verify_sentient_queue_entry, SIGNER_KEY_FILE,
};
pub use spend_ledger::{
    check_destination_allowed, release_legs, reserve_legs, LedgerView, ReserveOutcome, SpendLedger,
};
pub use stealth_pending::{
    consume_stealth_pending, record_stealth_pending, StealthPendingFile, STEALTH_PENDING_FILE,
};
pub use trader::{dry_run_from_env, ensure_local_signing_allowed, SentientTrader, SwapExecution};
