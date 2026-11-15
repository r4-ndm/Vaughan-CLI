//! Vaughan isolated signing daemon (`vaughan signerd`).
//!
//! Single purpose: module root, `Signer` struct, constructor/balance helpers,
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
pub mod broadcaster;
pub mod intents;
pub mod migrate;
mod ops;
mod policy;
pub mod quoter;
mod server;
mod sign;

pub use approval::{
    consume_approval, inspect_approval, inspect_approval_at, load_pending_policy,
    stage_pending_policy_with_replace, write_approval, Approval, ApprovalError, ApprovalPresence,
};
pub use broadcaster::{MockBroadcaster, TransactionBroadcaster, WalletBroadcaster};
pub use migrate::{check_legacy_policy_absent, run_migrate};
pub use quoter::{LiveSwapQuoter, MockSwapQuoter, SwapQuoter};
pub use server::run_signerd;
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
    pub(super) balance_overrides: Mutex<HashMap<Option<Address>, U256>>,
    pub(super) breaker: Mutex<vaughan_agent::CircuitBreaker>,
    pub(super) step_up_limiter: Mutex<HashMap<u32, Vec<Instant>>>,
    pub(super) expected_approval_uid: std::sync::atomic::AtomicU32,
}

/// Signer engine handling policy checks, ledger reservations, and calldata builds.
#[derive(Clone)]
pub struct Signer {
    pub(super) inner: Arc<SignerInner>,
}

impl Signer {
    /// Create a new Signer instance.
    pub fn new(
        store: PathBuf,
        active_address: Address,
        chain_id: u64,
        rpc_url: String,
        quoter: Arc<dyn SwapQuoter>,
        broadcaster: Arc<dyn TransactionBroadcaster>,
    ) -> Self {
        let rpc_count = 1usize;
        let config =
            vaughan_agent::breaker_config_for_session(Some(&store), rpc_count).unwrap_or_default();
        let breaker = vaughan_agent::CircuitBreaker::new(config);
        Self {
            inner: Arc::new(SignerInner {
                store,
                active_address,
                chain_id,
                rpc_url,
                quoter,
                broadcaster,
                balance_overrides: Mutex::new(HashMap::new()),
                breaker: Mutex::new(breaker),
                step_up_limiter: Mutex::new(HashMap::new()),
                expected_approval_uid: std::sync::atomic::AtomicU32::new(
                    approval::PRODUCTION_APPROVAL_UID,
                ),
            }),
        }
    }

    /// Override expected UID for `policy-approval.json` (used by non-root tests; production uses 0).
    pub fn set_expected_approval_uid(&self, uid: u32) {
        self.inner
            .expected_approval_uid
            .store(uid, std::sync::atomic::Ordering::Relaxed);
    }

    /// Access the circuit breaker inside the signer.
    pub fn breaker(&self) -> vaughan_agent::CircuitBreaker {
        self.inner.breaker.lock().unwrap().clone()
    }

    /// Set a balance override for an asset (token: None for native).
    pub fn set_balance(&self, token: Option<Address>, balance: U256) {
        self.inner
            .balance_overrides
            .lock()
            .unwrap()
            .insert(token, balance);
    }

    /// Get the current balance of an asset (returns override if set, or default).
    pub async fn get_balance(&self, token: Option<Address>) -> U256 {
        if let Some(b) = self.inner.balance_overrides.lock().unwrap().get(&token) {
            return *b;
        }
        // Default generous balance for test harness
        U256::from(10_000_000_000_000_000_000u128)
    }

    /// Query the balance of an on-chain address via RPC (or check balance overrides).
    pub async fn get_onchain_balance(&self, address: Address) -> Result<U256, String> {
        if let Some(b) = self
            .inner
            .balance_overrides
            .lock()
            .unwrap()
            .get(&Some(address))
        {
            return Ok(*b);
        }
        let url: alloy::transports::http::reqwest::Url = self
            .inner
            .rpc_url
            .parse()
            .map_err(|e| format!("invalid rpc_url: {e}"))?;
        let provider = alloy::providers::ProviderBuilder::new().connect_http(url);
        use alloy::providers::Provider;
        let balance = provider
            .get_balance(address)
            .await
            .map_err(|e| format!("eth_getBalance failed: {e}"))?;
        Ok(balance)
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
                None => {
                    self.handle_policy_update(
                        req.password,
                        req.policy,
                        req.expected_digest,
                        peer_uid,
                    )
                    .await
                }
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
                Some(intent) => self.handle_sign(intent).await,
                None => SignerResponse::error("missing_intent", "op 'sign' requires 'intent'"),
            },
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
