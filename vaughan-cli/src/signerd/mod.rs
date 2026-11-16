//! Vaughan isolated signing daemon (`vaughan signerd`).
//!
//! Single purpose: module root, `Signer` struct, constructor, the
//! `get_balance` fail-closed wrapper around the injected [`balance::BalanceSource`],
//! line dispatch, and `SO_PEERCRED` peer-UID helpers. Must NOT contain policy
//! update handlers, `handle_sign`, per-intent `ops` bodies, or the Unix socket
//! server accept loop (those live in `policy`, `sign`, `ops`, and `server`).
//!
//! # Process and Trust Model
//! The signer runs as a dedicated unprivileged UID (e.g. `vaughan-signer`),
//! separated from the agent's UID. Callers submit typed
//! [`vaughan_core::core::SignerIntent`] objects over a peer-UID-authenticated
//! Unix domain socket.
//!
//! # Security Invariant
//! Raw calldata never crosses the socket. Calldata is constructed inside this
//! process from verified, re-derived parameters. Policy enforcement and the spend
//! ledger reside exclusively inside the signer.

pub mod approval;
pub mod balance;
pub mod broadcaster;
mod gas;
pub mod intents;
pub mod migrate;
mod ops;
mod policy;
pub mod quoter;
mod server;
mod sign;
mod trip;

pub use approval::{
    consume_approval, inspect_approval, inspect_approval_at, load_pending_policy,
    stage_pending_policy_with_replace, write_approval, Approval, ApprovalError, ApprovalPresence,
};
pub use balance::{BalanceSource, LiveBalanceSource, MockBalanceSource};
pub use broadcaster::{FeeQuote, MockBroadcaster, TransactionBroadcaster, WalletBroadcaster};
pub use gas::SendPhase;
pub use migrate::{check_legacy_policy_absent, run_migrate};
pub use policy::{STEP_UP_UNSUPPORTED_CODE, STEP_UP_UNSUPPORTED_MSG};
pub use quoter::{LiveSwapQuoter, MockSwapQuoter, SwapQuoter};
pub use server::run_signerd;
pub use trip::TRIP_FILE;
pub use vaughan_core::core::signer_client::{
    PendingInfo, SignerClient, SignerRequest, SignerResponse,
};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};

/// Check if a peer UID retrieved from `SO_PEERCRED` is in the allowlist.
pub fn is_peer_uid_allowed(peer_uid: u32, allowlist: &[u32]) -> bool {
    allowlist.contains(&peer_uid)
}

/// Look up the numeric GID of the `vaughan` group in `/etc/group` (safe pure Rust).
#[cfg(unix)]
pub fn find_vaughan_gid() -> Option<u32> {
    if let Ok(content) = std::fs::read_to_string("/etc/group") {
        for line in content.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 3 && parts[0] == "vaughan" {
                if let Ok(gid) = parts[2].parse::<u32>() {
                    return Some(gid);
                }
            }
        }
    }
    None
}

pub(super) const STEP_UP_WINDOW: Duration = Duration::from_secs(60);
pub(super) const STEP_UP_MAX_ATTEMPTS: usize = 5;

pub(super) struct SignerInner {
    pub(super) store: PathBuf,
    pub(super) active_address: Address,
    pub(super) chain_id: u64,
    pub(super) rpc_url: String,
    pub(super) quoter: Arc<dyn SwapQuoter>,
    pub(super) broadcaster: Arc<dyn TransactionBroadcaster>,
    pub(super) balances: Arc<dyn BalanceSource>,
    pub(super) breaker: Mutex<vaughan_agent::CircuitBreaker>,
    pub(super) step_up_limiter: Mutex<HashMap<u32, Vec<Instant>>>,
    pub(super) expected_approval_uid: std::sync::atomic::AtomicU32,
    /// Operator opt-in for signing on a non-testnet chain; see [`Signer::allow_mainnet_writes`].
    pub(super) mainnet_writes_allowed: std::sync::atomic::AtomicBool,
}

/// Signer engine handling policy checks, ledger reservations, and calldata builds.
#[derive(Clone)]
pub struct Signer {
    pub(super) inner: Arc<SignerInner>,
}

/// Resolve `$token`'s balance via [`Signer::get_balance`], or `return` the
/// caller's `balance_unavailable` refusal immediately — one line at each
/// cap-sizing call site in `ops::*`, instead of a four-line `match`.
macro_rules! balance_or_refuse {
    ($self:expr, $token:expr) => {
        match $self.get_balance($token).await {
            Ok(b) => b,
            Err(resp) => return resp,
        }
    };
}
pub(super) use balance_or_refuse;

/// Resolve what a swap's input leg spends (`(balance token, ledger asset key)`, see
/// [`intents::swap_input_asset`]), or `return` a `wrapped_native_unconfigured` refusal
/// before any balance read, reservation or broadcast.
macro_rules! swap_input_or_refuse {
    ($chain_id:expr, $token_in:expr) => {
        match intents::swap_input_asset($chain_id, $token_in) {
            Ok(v) => v,
            Err(e) => return SignerResponse::error("wrapped_native_unconfigured", e),
        }
    };
}
pub(super) use swap_input_or_refuse;

impl Signer {
    /// Create a new Signer instance.
    pub fn new(
        store: PathBuf,
        active_address: Address,
        chain_id: u64,
        rpc_url: String,
        quoter: Arc<dyn SwapQuoter>,
        broadcaster: Arc<dyn TransactionBroadcaster>,
        balances: Arc<dyn BalanceSource>,
    ) -> Self {
        let rpc_count = 1usize;
        // A policy that cannot be read is NOT silently the default: the owner's ceilings may
        // be tighter than the defaults, so say so loudly — `status` reports what is actually
        // in force, and `handle_sign` re-reads the policy per request (see `replace_config`).
        let config = match vaughan_agent::breaker_config_for_session(Some(&store), rpc_count) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(
                    "could not load sentient policy from {}: {e} — falling back to the built-in \
                     breaker defaults; `status` reports the config in force",
                    store.display()
                );
                vaughan_agent::CircuitBreakerConfig::default()
            }
        };
        let breaker = vaughan_agent::CircuitBreaker::new(config);
        // Durable kill switch: a trip recorded by an earlier process (including one that
        // panicked or was OOM-killed and restarted by systemd) must still be halted here.
        match trip::load_trip(&store) {
            trip::TripOnDisk::Absent => {}
            trip::TripOnDisk::Tripped(reason) => {
                tracing::error!("signer daemon starting TRIPPED: {reason}");
                breaker.trip(reason);
            }
        }
        Self {
            inner: Arc::new(SignerInner {
                store,
                active_address,
                chain_id,
                rpc_url,
                quoter,
                broadcaster,
                balances,
                breaker: Mutex::new(breaker),
                step_up_limiter: Mutex::new(HashMap::new()),
                expected_approval_uid: std::sync::atomic::AtomicU32::new(
                    approval::PRODUCTION_APPROVAL_UID,
                ),
                mainnet_writes_allowed: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    /// Override expected UID for `policy-approval.json` (used by non-root tests; production uses 0).
    pub fn set_expected_approval_uid(&self, uid: u32) {
        self.inner
            .expected_approval_uid
            .store(uid, std::sync::atomic::Ordering::Relaxed);
    }

    /// Allow signing on a non-testnet chain (default: refused, `mainnet_blocked`).
    ///
    /// Set once at startup from the daemon's own environment (`VAUGHAN_MCP_ALLOW_MAINNET`,
    /// provided by the root-owned `/etc/vaughan/sentient.env`), never from a request: the
    /// agent that talks to the daemon must not be able to lift its own mainnet guard.
    pub fn allow_mainnet_writes(&self, allow: bool) {
        self.inner
            .mainnet_writes_allowed
            .store(allow, std::sync::atomic::Ordering::Relaxed);
    }

    /// Access the circuit breaker inside the signer.
    pub fn breaker(&self) -> vaughan_agent::CircuitBreaker {
        self.inner.breaker.lock().unwrap().clone()
    }

    /// Read the active account's REAL on-chain balance of `token` (native if `None`).
    ///
    /// Every spend cap is sized against this value. A read failure fails closed:
    /// never a cached, default, or zero balance — the caller must refuse the
    /// intent with `balance_unavailable` rather than reserve or broadcast.
    pub(super) async fn get_balance(&self, token: Option<Address>) -> Result<U256, SignerResponse> {
        self.inner
            .balances
            .balance_of(&self.inner.rpc_url, self.inner.active_address, token)
            .await
            .map_err(|e| {
                SignerResponse::error(
                    "balance_unavailable",
                    format!("could not read balance: {e}"),
                )
            })
    }

    /// Handle a newline-delimited JSON line and return the JSON response string.
    pub async fn handle_line(&self, line: &str) -> String {
        self.handle_line_with_uid(line, 0).await
    }

    /// Handle a newline-delimited JSON line for an explicit peer UID and return the JSON response string.
    pub async fn handle_line_with_uid(&self, line: &str, peer_uid: u32) -> String {
        let resp = self.handle_line_to_response_with_uid(line, peer_uid).await;
        serde_json::to_string(&resp).unwrap_or_else(|e| {
            format!(r#"{{"ok":false,"code":"internal_error","message":"{e}"}}"#)
        })
    }

    /// Parse and dispatch a single JSON line to `SignerResponse`.
    pub async fn handle_line_to_response(&self, line: &str) -> SignerResponse {
        self.handle_line_to_response_with_uid(line, 0).await
    }

    /// Parse and dispatch a single JSON line to `SignerResponse` with explicit peer UID rate limiting.
    pub async fn handle_line_to_response_with_uid(
        &self,
        line: &str,
        peer_uid: u32,
    ) -> SignerResponse {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return SignerResponse::error("empty_request", "empty request line");
        }
        let req: SignerRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                return SignerResponse::error("invalid_json", format!("invalid request JSON: {e}"));
            }
        };

        match req.op.as_str() {
            "status" => self.handle_status(),
            "policy" => self.handle_policy(),
            "policy_pending" => self.handle_policy_pending(),
            "policy_update" => match req.auth.as_deref() {
                None => self.handle_policy_unsupported(),
                Some("stage") => self.handle_policy_stage(req.policy, req.expected_digest),
                Some("approved") => {
                    self.handle_policy_approved(req.policy, req.expected_digest, peer_uid)
                }
                Some(_) => SignerResponse::error(
                    approval::STEP_UP_FAILED_CODE,
                    approval::STEP_UP_FAILED_MSG,
                ),
            },
            "sign" => match req.intent {
                Some(intent) => {
                    let resp = self.handle_sign(intent).await;
                    // An automatic trip (gas ceiling / error streak) raised while handling
                    // this request must outlive a restart too, not just this process.
                    self.persist_automatic_trip();
                    resp
                }
                None => SignerResponse::error("missing_intent", "op 'sign' requires 'intent'"),
            },
            "trip" => self.handle_trip(req.reason, peer_uid),
            other => SignerResponse::error("unknown_op", format!("unrecognized op '{other}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_uid_allowlist_accept_and_refuse() {
        let allowlist = vec![1000, 1001];

        // Allowed UIDs
        assert!(is_peer_uid_allowed(1000, &allowlist));
        assert!(is_peer_uid_allowed(1001, &allowlist));

        // Refused UIDs
        assert!(!is_peer_uid_allowed(1002, &allowlist));
        assert!(!is_peer_uid_allowed(0, &allowlist)); // root must not be implicitly accepted
        assert!(!is_peer_uid_allowed(65534, &allowlist)); // nobody

        // Empty allowlist accepts nothing
        assert!(!is_peer_uid_allowed(1000, &[]));
    }
}
