//! Vaughan Agent: proposal engine, tool registry, and Sentient circuit breakers.
//!
//! Library-only crate for future MCP integration. Embedded in-wallet LLM chat
//! was retired in 2026-08; this crate exposes structured tools and safety
//! boundaries for external agents.

pub mod dca;
pub mod error;
pub mod paths;
pub mod presets;
pub mod proposal;
pub mod sentient;
pub mod tools;
pub mod types;

pub use dca::{
    add_plan, agg_picker_labels, build_plan, build_proposal, cancel_plan, dex_picker_slugs,
    fire_plan, load_plans, now_unix, poll_due_id, resolve_plan_pair_and_corridor, set_paused,
    DcaFireResult, DcaPlan, DcaPlanFile, DcaPlanProposal, DcaStatus, DcaTrigger, DcaVenue,
    MIN_INTERVAL_SECS,
};
pub use error::AgentError;
pub use paths::{
    assert_signer_store_writable, is_signer_mode_active, profile_dir, set_signer_mode_override,
    set_signer_store_override, SignerStore, SignerStoreError, ENV_SIGNER_MODE, ENV_SIGNER_STORE,
};
pub use presets::{apply_preset, presets_root, BUNDLED_PRESET_IDS};
pub use proposal::{DexSwapKind, ProposalType, TxProposal};
pub use sentient::{
    assert_owner_only_file, breaker_config_for_session, build_policy_proposal,
    check_destination_allowed, consume_stealth_pending, derive_sentient_queue_key,
    dry_run_from_env, ensure_local_signing_allowed, init_signer_key, load_policy, load_signer_key,
    record_stealth_pending, release_legs, reserve_legs, save_policy, sentient_queue_hmac_key,
    verify_sentient_queue_entry, AgentSessionPolicy, CircuitBreaker, CircuitBreakerConfig,
    EnforcementMode, LedgerView, PolicyProposal, QuorumReserves, QuorumValidator, ReserveOutcome,
    SentientTrader, SpendLedger, SwapExecution, LEGACY_DEGEN_POLICY_TOML, SENTIENT_POLICY_TOML,
    SIGNER_KEY_FILE,
};
pub use tools::{
    default_assist_registry, default_assist_registry_for, default_sensory_registry,
    default_sentient_registry, Tool, ToolContext, ToolRegistry,
};
pub use types::ToolDefinition;
