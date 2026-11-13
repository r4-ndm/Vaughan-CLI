//! Vaughan isolated signing daemon (`vaughan signerd`).
//!
//! # Process and Trust Model
//! The signer runs as a dedicated unprivileged UID (e.g. `vaughan-signer`),
//! separated from the agent's UID. Callers submit typed [`SignerIntent`] objects
//! over a peer-UID-authenticated Unix domain socket.
//!
//! # Security Invariant
//! Raw calldata never crosses the socket. Calldata is constructed inside this
//! process from verified, re-derived parameters. Policy enforcement and the spend
//! ledger reside exclusively inside the signer.

pub mod broadcaster;
pub mod intents;
pub mod migrate;
pub mod quoter;

pub use broadcaster::{MockBroadcaster, TransactionBroadcaster, WalletBroadcaster};
pub use migrate::{check_legacy_policy_absent, run_migrate};
pub use quoter::{LiveSwapQuoter, MockSwapQuoter, SwapQuoter};
pub use vaughan_core::core::signer_client::{SignerClient, SignerRequest, SignerResponse};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};
use secrecy::{ExposeSecret, SecretString};
use tokio::io::AsyncWriteExt;
use vaughan_agent::sentient::policy::EnforcementMode;
use vaughan_agent::sentient::SpendLedger;
use vaughan_core::core::{OperatingMode, SignerIntent, SwapFamily, WalletState};
use zeroize::Zeroize;

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

const STEP_UP_WINDOW: Duration = Duration::from_secs(60);
const STEP_UP_MAX_ATTEMPTS: usize = 5;

struct SignerInner {
    store: PathBuf,
    active_address: Address,
    chain_id: u64,
    rpc_url: String,
    quoter: Arc<dyn SwapQuoter>,
    broadcaster: Arc<dyn TransactionBroadcaster>,
    balance_overrides: Mutex<HashMap<Option<Address>, U256>>,
    breaker: Mutex<vaughan_agent::CircuitBreaker>,
    step_up_limiter: Mutex<HashMap<u32, Vec<Instant>>>,
}

/// Signer engine handling policy checks, ledger reservations, and calldata builds.
#[derive(Clone)]
pub struct Signer {
    inner: Arc<SignerInner>,
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
            }),
        }
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
            "policy_update" => {
                self.handle_policy_update(req.password, req.policy, req.expected_digest, peer_uid)
                    .await
            }
            "sign" => match req.intent {
                Some(intent) => self.handle_sign(intent).await,
                None => SignerResponse::error("missing_intent", "op 'sign' requires 'intent'"),
            },
            other => SignerResponse::error("unknown_op", format!("unrecognized op '{other}'")),
        }
    }

    /// Handle `{"op":"status"}` without requiring any unlock beyond what daemon holds.
    pub fn handle_status(&self) -> SignerResponse {
        let policy = match vaughan_agent::sentient::load_policy(&self.inner.store) {
            Ok(p) => p,
            Err(e) => {
                return SignerResponse::error(
                    "policy_load_failed",
                    format!("could not load policy: {e}"),
                );
            }
        };

        let view = match vaughan_agent::sentient::SpendLedger::snapshot(&self.inner.store) {
            Ok(v) => v,
            Err(e) => {
                return SignerResponse::error(
                    "spend_ledger_unreadable",
                    format!("could not load spend ledger: {e}"),
                );
            }
        };

        let ledger_path = self.inner.store.join("sentient-spend.json");
        let window_reset_unix = if view.window_start_unix == 0 {
            0
        } else {
            view.window_start_unix.saturating_add(view.window_seconds)
        };

        let mut assets_out = Vec::new();
        let mut asset_keys: Vec<_> = view.assets.keys().collect();
        asset_keys.sort();
        for key in asset_keys {
            let item = &view.assets[key];
            let cap = item
                .snapshot
                .saturating_mul(alloy::primitives::U256::from(policy.daily_cap_pct))
                .checked_div(alloy::primitives::U256::from(100))
                .unwrap_or(alloy::primitives::U256::ZERO);
            let remaining = cap.saturating_sub(item.spent);
            assets_out.push(serde_json::json!({
                "asset": key,
                "snapshot": item.snapshot.to_string(),
                "spent": item.spent.to_string(),
                "cap": cap.to_string(),
                "remaining": remaining.to_string(),
            }));
        }

        let is_tripped = self.inner.breaker.lock().unwrap().is_tripped();
        let trip_reason = self.inner.breaker.lock().unwrap().trip_reason();

        let mut budget_json = serde_json::json!({
            "profile": "sentient",
            "chain_id": self.inner.chain_id,
            "enforcement": policy.enforcement.as_str(),
            "daily_cap_pct": policy.daily_cap_pct,
            "window_reset_unix": window_reset_unix,
            "assets": assets_out,
            "allow_transfer_to": policy.allow_transfer_to,
            "ledger_path": ledger_path.display().to_string(),
            "circuit_breaker_tripped": is_tripped,
        });
        if let Some(reason) = trip_reason {
            budget_json["circuit_breaker_reason"] = serde_json::json!(reason);
        }
        if view.assets.is_empty() {
            budget_json["note"] = serde_json::json!("no spend recorded this window");
        }

        let mut resp = SignerResponse::status(
            self.inner.active_address,
            self.inner.chain_id,
            self.inner.store.display().to_string(),
            policy.digest(),
        );
        resp.budget = Some(budget_json);
        resp
    }

    /// Handle `{"op":"policy"}` (read-only, no authentication).
    pub fn handle_policy(&self) -> SignerResponse {
        let policy = match vaughan_agent::sentient::load_policy(&self.inner.store) {
            Ok(p) => p,
            Err(e) => {
                return SignerResponse::error(
                    "policy_load_failed",
                    format!("could not load policy: {e}"),
                );
            }
        };
        let digest = policy.digest();
        match serde_json::to_value(&policy) {
            Ok(val) => SignerResponse::policy_ok(val, digest),
            Err(e) => {
                SignerResponse::error("internal_error", format!("could not serialize policy: {e}"))
            }
        }
    }

    /// Handle `{"op":"policy_update","password":"…","policy":{…}}`.
    ///
    /// Rules:
    /// 1. Verify vault password via `spawn_blocking` KDF before touching or accepting any policy.
    /// 2. Rate-limit step-up attempts per peer UID (5 attempts per 60s window).
    /// 3. Return indistinguishable `step_up_failed` on wrong password, missing wallet, or corrupt store.
    /// 4. Enforce `expected_digest` if supplied (`policy_conflict`).
    /// 5. Refuse `enforcement:"disabled"` unless persisted on-disk policy already has `acknowledge_unsafe: true`.
    /// 6. Run canonical `AgentSessionPolicy::validate()`.
    /// 7. Atomically persist via `save_policy` and reload breaker config.
    pub async fn handle_policy_update(
        &self,
        password: Option<SecretString>,
        policy_val: Option<serde_json::Value>,
        expected_digest: Option<String>,
        peer_uid: u32,
    ) -> SignerResponse {
        let Some(password) = password else {
            return SignerResponse::error(
                "step_up_failed",
                "vault password is required for policy_update",
            );
        };
        if password.expose_secret().is_empty() {
            return SignerResponse::error(
                "step_up_failed",
                "vault password is required for policy_update",
            );
        }

        // Per-peer-UID backoff before running Argon2id in spawn_blocking.
        {
            let mut limiter = self.inner.step_up_limiter.lock().unwrap();
            let now = Instant::now();
            let entries = limiter.entry(peer_uid).or_default();
            entries.retain(|t| now.duration_since(*t) < STEP_UP_WINDOW);
            if entries.len() >= STEP_UP_MAX_ATTEMPTS {
                return SignerResponse::error(
                    "step_up_rate_limited",
                    "rate limit exceeded: max 5 step-up attempts per minute",
                );
            }
            entries.push(now);
        }

        let wallet_path = self.inner.store.join("wallet.json");
        let verify_res = tokio::task::spawn_blocking(move || {
            let mut wallet = WalletState::load_with_session(
                wallet_path,
                OperatingMode::SentientTrader,
                "sentient",
            )
            .map_err(|_| ())?;
            wallet.unlock(&password).map_err(|_| ())?;
            Ok::<(), ()>(())
        })
        .await;

        match verify_res {
            Ok(Ok(())) => {
                let mut limiter = self.inner.step_up_limiter.lock().unwrap();
                if let Some(entries) = limiter.get_mut(&peer_uid) {
                    entries.pop();
                }
            }
            _ => {
                return SignerResponse::error(
                    "step_up_failed",
                    "vault password verification failed",
                );
            }
        }

        let old_policy = match vaughan_agent::sentient::load_policy(&self.inner.store) {
            Ok(p) => p,
            Err(e) => {
                return SignerResponse::error(
                    "policy_load_failed",
                    format!("could not load current policy: {e}"),
                );
            }
        };

        if let Some(expected) = expected_digest {
            let current_digest = old_policy.digest();
            if expected != current_digest {
                return SignerResponse::error(
                    "policy_conflict",
                    format!(
                        "policy digest mismatch: expected {expected}, current is {current_digest}"
                    ),
                );
            }
        }

        let Some(val) = policy_val else {
            return SignerResponse::error("policy_invalid", "missing 'policy' object");
        };

        let new_policy: vaughan_agent::sentient::AgentSessionPolicy =
            match serde_json::from_value(val) {
                Ok(p) => p,
                Err(e) => {
                    return SignerResponse::error(
                        "policy_invalid",
                        format!("invalid policy payload: {e}"),
                    );
                }
            };

        if new_policy.enforcement == EnforcementMode::Disabled
            && (!old_policy.acknowledge_unsafe || !new_policy.acknowledge_unsafe)
        {
            return SignerResponse::error(
                "policy_refused",
                "enforcement=disabled requires acknowledge_unsafe=true already persisted on disk (two-step confirm-unsafe required)",
            );
        }

        if let Err(e) = new_policy.validate() {
            return SignerResponse::error("policy_invalid", e.to_string());
        }

        if let Err(e) = vaughan_agent::sentient::save_policy(&self.inner.store, &new_policy) {
            return SignerResponse::error("policy_save_failed", e.to_string());
        }

        if let Err(e) = vaughan_agent::sentient::append_policy_audit(
            &self.inner.store,
            &old_policy,
            &new_policy,
        ) {
            let _ = vaughan_agent::sentient::save_policy(&self.inner.store, &old_policy);
            return SignerResponse::error("policy_audit_failed", e.to_string());
        }

        if let Ok(cfg) = vaughan_agent::breaker_config_for_session(Some(&self.inner.store), 1) {
            self.inner.breaker.lock().unwrap().replace_config(cfg);
        }

        SignerResponse::policy_update_ok(new_policy.digest())
    }

    /// Handle `{"op":"sign","intent":{…}}`.
    pub async fn handle_sign(&self, intent: SignerIntent) -> SignerResponse {
        // 0. Circuit breaker trip check
        if self.inner.breaker.lock().unwrap().is_tripped() {
            let reason = self
                .inner
                .breaker
                .lock()
                .unwrap()
                .trip_reason()
                .unwrap_or_else(|| "halted".into());
            return SignerResponse::error(
                "circuit_breaker_tripped",
                format!("circuit breaker tripped: {reason}"),
            );
        }

        // 1. Chain ID match
        if intent.chain_id() != self.inner.chain_id {
            return SignerResponse::error(
                "chain_id_mismatch",
                format!(
                    "intent chain_id {} does not match signer active chain_id {}",
                    intent.chain_id(),
                    self.inner.chain_id
                ),
            );
        }

        // 2. Load active policy
        let policy = match vaughan_agent::sentient::load_policy(&self.inner.store) {
            Ok(p) => p,
            Err(e) => {
                return SignerResponse::error(
                    "policy_load_failed",
                    format!("could not load policy: {e}"),
                );
            }
        };

        // 3. Dispatch by intent variant
        match intent {
            SignerIntent::Transfer {
                chain_id,
                to,
                token,
                amount,
            } => {
                self.handle_transfer(chain_id, to, token, amount, &policy)
                    .await
            }

            SignerIntent::Swap {
                chain_id,
                family,
                router,
                path,
                amount_in,
                min_amount_out,
                fee,
            } => {
                self.handle_swap(
                    chain_id,
                    family,
                    router,
                    path,
                    amount_in,
                    min_amount_out,
                    fee,
                    &policy,
                )
                .await
            }

            SignerIntent::Approve {
                chain_id,
                token,
                spender,
                amount,
                expiry,
            } => {
                self.handle_approve(chain_id, token, spender, amount, expiry, &policy)
                    .await
            }

            SignerIntent::Wrap { chain_id, amount } => {
                self.handle_wrap(chain_id, amount, &policy).await
            }

            SignerIntent::Unwrap { chain_id, amount } => {
                self.handle_unwrap(chain_id, amount, &policy).await
            }

            SignerIntent::BatchTransfer { chain_id, legs } => {
                self.handle_batch_transfer(chain_id, legs, &policy).await
            }

            SignerIntent::DcaSlice {
                plan_id,
                slice_index,
                chain_id,
                pair,
                amount_in,
                corridor_min_out,
            } => {
                self.handle_dca_slice(
                    plan_id,
                    slice_index,
                    chain_id,
                    pair,
                    amount_in,
                    corridor_min_out,
                    &policy,
                )
                .await
            }

            SignerIntent::SweepStealth {
                chain_id,
                stealth_address,
            } => {
                self.handle_sweep_stealth(chain_id, stealth_address, &policy)
                    .await
            }
        }
    }

    async fn handle_transfer(
        &self,
        chain_id: u64,
        to: Address,
        token: Option<Address>,
        amount: U256,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        // Enforce allow_transfer_to
        if let Err(e) = vaughan_agent::sentient::check_destination_allowed(policy, to) {
            return SignerResponse::error("destination_not_allowed", e.to_string());
        }

        let balance = self.get_balance(token).await;
        let cap = (balance * U256::from(policy.max_position_pct)) / U256::from(100);
        if policy.enforcement == EnforcementMode::Enforced && amount > cap {
            return SignerResponse::error(
                "position_cap_exceeded",
                format!(
                    "transfer amount {amount} exceeds max_position_pct ({}%) cap of {cap}",
                    policy.max_position_pct
                ),
            );
        }

        let asset = match token {
            None => format!("{chain_id}:native"),
            Some(t) => format!("{chain_id}:{t:#x}"),
        };
        let intent = SignerIntent::Transfer {
            chain_id,
            to,
            token,
            amount,
        };
        let key = intent.idempotency_key();

        // Reserve spend in the ledger
        match SpendLedger::reserve(
            &self.inner.store,
            &key,
            &asset,
            amount,
            balance,
            policy.daily_cap_pct,
            policy.enforcement,
        ) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                let code = if msg.contains("spend_already_reserved") {
                    "spend_already_reserved"
                } else {
                    "spend_guard"
                };
                return SignerResponse::error(code, msg);
            }
        }

        // Build calldata inside signer
        let tx = intents::build_transfer_tx(chain_id, self.inner.active_address, to, token, amount);

        // Sign and broadcast
        match self.inner.broadcaster.broadcast(tx).await {
            Ok(tx_hash) => {
                let _ = self
                    .inner
                    .breaker
                    .lock()
                    .unwrap()
                    .record_success(U256::ZERO);
                SignerResponse::sign_ok(tx_hash)
            }
            Err(err) => {
                self.inner.breaker.lock().unwrap().record_failure(&err);
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                SignerResponse::error("broadcast_failed", err)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_swap(
        &self,
        chain_id: u64,
        family: SwapFamily,
        router: Address,
        path: Vec<Address>,
        amount_in: U256,
        min_amount_out: U256,
        fee: Option<u32>,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        // Enforce router allowlist per swap family
        let router_allowed = match family {
            SwapFamily::V2 => vaughan_core::core::is_allowed_dex_v2_router(chain_id, router),
            SwapFamily::V3 => vaughan_core::core::is_allowed_dex_v3_router(chain_id, router),
            SwapFamily::Agg => vaughan_core::core::is_allowed_agg_router_on_chain(chain_id, router),
        };
        if !router_allowed {
            return SignerResponse::error(
                "router_not_allowed",
                format!(
                    "router {router:#x} is not on the audited allowlist for {family:?} on chain {chain_id}"
                ),
            );
        }

        if min_amount_out.is_zero() {
            return SignerResponse::error(
                "zero_min_amount_out",
                "swap requires min_amount_out > 0 (fail-closed against unbounded slippage)",
            );
        }

        if path.len() < 2 {
            return SignerResponse::error(
                "invalid_swap_path",
                "swap path must have at least 2 tokens",
            );
        }

        // Re-derive quote live
        let expected_out = match family {
            SwapFamily::V2 => {
                match self
                    .inner
                    .quoter
                    .quote_v2(&self.inner.rpc_url, router, amount_in, &path)
                    .await
                {
                    Ok(out) => out,
                    Err(e) => {
                        return SignerResponse::error(
                            "quote_failed",
                            format!("fresh V2 quote failed: {e}"),
                        );
                    }
                }
            }
            SwapFamily::V3 => {
                let fee_val = fee.unwrap_or(3000);
                match self
                    .inner
                    .quoter
                    .quote_v3(
                        &self.inner.rpc_url,
                        chain_id,
                        path[0],
                        path[1],
                        fee_val,
                        amount_in,
                    )
                    .await
                {
                    Ok((_pool, out)) => out,
                    Err(e) => {
                        return SignerResponse::error(
                            "quote_failed",
                            format!("fresh V3 quote failed: {e}"),
                        );
                    }
                }
            }
            SwapFamily::Agg => min_amount_out,
        };

        if expected_out.is_zero() {
            return SignerResponse::error(
                "zero_quote",
                "re-derived swap quote is zero — auto-exec refuses unbounded swap",
            );
        }

        // Check slippage floor
        let max_bps = u64::from(policy.max_slippage_bps.min(10_000));
        let floor = expected_out * U256::from(10_000u64 - max_bps) / U256::from(10_000u64);
        if min_amount_out < floor {
            return SignerResponse::error(
                "slippage_exceeded",
                format!(
                    "min_amount_out {min_amount_out} is below policy floor {floor} (fresh quote {expected_out} less {max_bps} bps slippage)"
                ),
            );
        }

        let token_in = path[0];
        let asset = format!("{chain_id}:{token_in:#x}");
        let balance = self.get_balance(Some(token_in)).await;
        let intent = SignerIntent::Swap {
            chain_id,
            family,
            router,
            path: path.clone(),
            amount_in,
            min_amount_out,
            fee,
        };
        let key = intent.idempotency_key();

        match SpendLedger::reserve(
            &self.inner.store,
            &key,
            &asset,
            amount_in,
            balance,
            policy.daily_cap_pct,
            policy.enforcement,
        ) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                let code = if msg.contains("spend_already_reserved") {
                    "spend_already_reserved"
                } else {
                    "spend_guard"
                };
                return SignerResponse::error(code, msg);
            }
        }

        // Build calldata inside signer
        let tx = match family {
            SwapFamily::V2 => intents::build_v2_swap_tx(
                chain_id,
                self.inner.active_address,
                router,
                path,
                amount_in,
                min_amount_out,
                self.inner.active_address,
            ),
            SwapFamily::V3 => intents::build_v3_swap_tx(
                chain_id,
                self.inner.active_address,
                router,
                path[0],
                path[1],
                fee.unwrap_or(3000),
                amount_in,
                min_amount_out,
                self.inner.active_address,
            ),
            SwapFamily::Agg => intents::build_v2_swap_tx(
                chain_id,
                self.inner.active_address,
                router,
                path,
                amount_in,
                min_amount_out,
                self.inner.active_address,
            ),
        };
        let tx = match tx {
            Ok(t) => t,
            Err(e) => {
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                return SignerResponse::error("swap_failed", e);
            }
        };

        match self.inner.broadcaster.broadcast(tx).await {
            Ok(tx_hash) => {
                let _ = self
                    .inner
                    .breaker
                    .lock()
                    .unwrap()
                    .record_success(U256::ZERO);
                SignerResponse::sign_ok(tx_hash)
            }
            Err(err) => {
                self.inner.breaker.lock().unwrap().record_failure(&err);
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                SignerResponse::error("broadcast_failed", err)
            }
        }
    }

    async fn handle_approve(
        &self,
        chain_id: u64,
        token: Address,
        spender: Address,
        amount: U256,
        expiry: Option<u64>,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        // Refuse U256::MAX approvals
        if amount == U256::MAX {
            return SignerResponse::error(
                "approve_max_refused",
                "unlimited approve (U256::MAX) is refused by policy",
            );
        }

        // Check spender allowlist
        let spender_ok = vaughan_core::core::is_allowed_dex_router(chain_id, spender)
            || vaughan_core::core::is_allowed_agg_router_on_chain(chain_id, spender)
            || vaughan_agent::sentient::check_destination_allowed(policy, spender).is_ok();
        if !spender_ok {
            return SignerResponse::error(
                "spender_not_allowed",
                format!("spender {spender:#x} is not on the router or transfer allowlist"),
            );
        }

        // Check expiry if set
        if let Some(exp) = expiry {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if exp > 0 && exp <= now {
                return SignerResponse::error(
                    "intent_expired",
                    format!("approve intent expired at {exp}"),
                );
            }
        }

        // Check approve cap
        let balance = self.get_balance(Some(token)).await;
        let cap_pct = policy.max_approve_pct();
        let cap = (balance * U256::from(cap_pct)) / U256::from(100);
        if policy.enforcement == EnforcementMode::Enforced && amount > cap {
            return SignerResponse::error(
                "approve_cap_exceeded",
                format!(
                    "approval amount {amount} exceeds max_approve_pct ({cap_pct}%) cap of {cap}"
                ),
            );
        }

        let asset = format!("{chain_id}:{token:#x}");
        let intent = SignerIntent::Approve {
            chain_id,
            token,
            spender,
            amount,
            expiry,
        };
        let key = intent.idempotency_key();

        match SpendLedger::reserve(
            &self.inner.store,
            &key,
            &asset,
            amount,
            balance,
            policy.daily_cap_pct,
            policy.enforcement,
        ) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                let code = if msg.contains("spend_already_reserved") {
                    "spend_already_reserved"
                } else {
                    "spend_guard"
                };
                return SignerResponse::error(code, msg);
            }
        }

        let tx =
            intents::build_approve_tx(chain_id, self.inner.active_address, token, spender, amount);

        match self.inner.broadcaster.broadcast(tx).await {
            Ok(tx_hash) => {
                let _ = self
                    .inner
                    .breaker
                    .lock()
                    .unwrap()
                    .record_success(U256::ZERO);
                SignerResponse::sign_ok(tx_hash)
            }
            Err(err) => {
                self.inner.breaker.lock().unwrap().record_failure(&err);
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                SignerResponse::error("broadcast_failed", err)
            }
        }
    }

    async fn handle_wrap(
        &self,
        chain_id: u64,
        amount: U256,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        let asset = format!("{chain_id}:native");
        let balance = self.get_balance(None).await;
        let intent = SignerIntent::Wrap { chain_id, amount };
        let key = intent.idempotency_key();

        match SpendLedger::reserve(
            &self.inner.store,
            &key,
            &asset,
            amount,
            balance,
            policy.daily_cap_pct,
            policy.enforcement,
        ) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                let code = if msg.contains("spend_already_reserved") {
                    "spend_already_reserved"
                } else {
                    "spend_guard"
                };
                return SignerResponse::error(code, msg);
            }
        }

        let tx = match intents::build_wrap_tx(chain_id, self.inner.active_address, amount) {
            Ok(t) => t,
            Err(e) => {
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                return SignerResponse::error("wrap_failed", e);
            }
        };

        match self.inner.broadcaster.broadcast(tx).await {
            Ok(tx_hash) => {
                let _ = self
                    .inner
                    .breaker
                    .lock()
                    .unwrap()
                    .record_success(U256::ZERO);
                SignerResponse::sign_ok(tx_hash)
            }
            Err(err) => {
                self.inner.breaker.lock().unwrap().record_failure(&err);
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                SignerResponse::error("broadcast_failed", err)
            }
        }
    }

    async fn handle_unwrap(
        &self,
        chain_id: u64,
        amount: U256,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        let weth = match intents::resolve_wrapped_native(chain_id) {
            Ok(w) => w,
            Err(e) => return SignerResponse::error("unwrap_failed", e),
        };
        let asset = format!("{chain_id}:{weth:#x}");
        let balance = self.get_balance(Some(weth)).await;
        let intent = SignerIntent::Unwrap { chain_id, amount };
        let key = intent.idempotency_key();

        match SpendLedger::reserve(
            &self.inner.store,
            &key,
            &asset,
            amount,
            balance,
            policy.daily_cap_pct,
            policy.enforcement,
        ) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                let code = if msg.contains("spend_already_reserved") {
                    "spend_already_reserved"
                } else {
                    "spend_guard"
                };
                return SignerResponse::error(code, msg);
            }
        }

        let tx = match intents::build_unwrap_tx(chain_id, self.inner.active_address, amount) {
            Ok(t) => t,
            Err(e) => {
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                return SignerResponse::error("unwrap_failed", e);
            }
        };

        match self.inner.broadcaster.broadcast(tx).await {
            Ok(tx_hash) => {
                let _ = self
                    .inner
                    .breaker
                    .lock()
                    .unwrap()
                    .record_success(U256::ZERO);
                SignerResponse::sign_ok(tx_hash)
            }
            Err(err) => {
                self.inner.breaker.lock().unwrap().record_failure(&err);
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                SignerResponse::error("broadcast_failed", err)
            }
        }
    }

    async fn handle_batch_transfer(
        &self,
        chain_id: u64,
        legs: Vec<vaughan_core::core::BatchTransferLeg>,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        for leg in &legs {
            if let Err(e) = vaughan_agent::sentient::check_destination_allowed(policy, leg.to) {
                return SignerResponse::error("destination_not_allowed", e.to_string());
            }
        }

        let total_amount = legs
            .iter()
            .fold(U256::ZERO, |acc, l| acc.saturating_add(l.amount));
        let asset = format!("{chain_id}:native");
        let balance = self.get_balance(None).await;
        let intent = SignerIntent::BatchTransfer {
            chain_id,
            legs: legs.clone(),
        };
        let key = intent.idempotency_key();

        match SpendLedger::reserve(
            &self.inner.store,
            &key,
            &asset,
            total_amount,
            balance,
            policy.daily_cap_pct,
            policy.enforcement,
        ) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                let code = if msg.contains("spend_already_reserved") {
                    "spend_already_reserved"
                } else {
                    "spend_guard"
                };
                return SignerResponse::error(code, msg);
            }
        }

        let tx = intents::build_batch_transfer_tx(chain_id, self.inner.active_address, &legs);

        match self.inner.broadcaster.broadcast(tx).await {
            Ok(tx_hash) => {
                let _ = self
                    .inner
                    .breaker
                    .lock()
                    .unwrap()
                    .record_success(U256::ZERO);
                SignerResponse::sign_ok(tx_hash)
            }
            Err(err) => {
                self.inner.breaker.lock().unwrap().record_failure(&err);
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                SignerResponse::error("broadcast_failed", err)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_dca_slice(
        &self,
        plan_id: String,
        slice_index: u64,
        chain_id: u64,
        pair: Address,
        amount_in: U256,
        corridor_min_out: U256,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        if corridor_min_out.is_zero() {
            return SignerResponse::error(
                "zero_corridor_min_out",
                "corridor_min_out is zero — auto-exec refuses unbounded swap with no minimum output protection",
            );
        }

        if pair == Address::ZERO {
            return SignerResponse::error(
                "unknown_swap_kind",
                "dca_slice pair is zero address (fail-closed)",
            );
        }

        // Re-derive quote live from pool state; DexSwapKind rules apply
        let dca_quote = match self
            .inner
            .quoter
            .quote_dca(&self.inner.rpc_url, chain_id, pair, amount_in)
            .await
        {
            Ok(q) => q,
            Err(e) => {
                return SignerResponse::error(
                    "quote_failed",
                    format!("fresh DCA quote failed: {e}"),
                );
            }
        };

        if dca_quote.kind == vaughan_core::core::proposal::DexSwapKind::Unknown {
            return SignerResponse::error(
                "unknown_swap_kind",
                "unknown swap kind: pair is neither a V2 pair nor V3 pool (fail-closed)",
            );
        }

        if dca_quote.expected_out.is_zero() {
            return SignerResponse::error(
                "zero_quote",
                "re-derived swap quote is zero — auto-exec refuses unbounded swap",
            );
        }

        if dca_quote.expected_out < corridor_min_out {
            return SignerResponse::error(
                "quote_below_corridor",
                format!(
                    "re-derived quote {} is below corridor_min_out {}",
                    dca_quote.expected_out, corridor_min_out
                ),
            );
        }

        let asset = format!("{chain_id}:native");
        let balance = self.get_balance(None).await;
        let intent = SignerIntent::DcaSlice {
            plan_id,
            slice_index,
            chain_id,
            pair,
            amount_in,
            corridor_min_out,
        };
        let key = intent.idempotency_key();

        match SpendLedger::reserve(
            &self.inner.store,
            &key,
            &asset,
            amount_in,
            balance,
            policy.daily_cap_pct,
            policy.enforcement,
        ) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                let code = if msg.contains("spend_already_reserved") {
                    "spend_already_reserved"
                } else {
                    "spend_guard"
                };
                return SignerResponse::error(code, msg);
            }
        }

        // Build calldata inside signer enforcing encoded amountOutMinimum equals corridor value
        let tx = match match dca_quote.kind {
            vaughan_core::core::proposal::DexSwapKind::V3 { fee, .. } => intents::build_v3_swap_tx(
                chain_id,
                self.inner.active_address,
                dca_quote.router,
                dca_quote.token_in,
                dca_quote.token_out,
                fee,
                amount_in,
                corridor_min_out,
                self.inner.active_address,
            ),
            vaughan_core::core::proposal::DexSwapKind::V2 => intents::build_v2_swap_tx(
                chain_id,
                self.inner.active_address,
                dca_quote.router,
                dca_quote.path,
                amount_in,
                corridor_min_out,
                self.inner.active_address,
            ),
            _ => {
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                return SignerResponse::error(
                    "unknown_swap_kind",
                    "unexpected swap kind after quote validation (fail-closed)",
                );
            }
        } {
            Ok(t) => t,
            Err(e) => {
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                return SignerResponse::error("swap_failed", e);
            }
        };

        match self.inner.broadcaster.broadcast(tx).await {
            Ok(tx_hash) => {
                let _ = self
                    .inner
                    .breaker
                    .lock()
                    .unwrap()
                    .record_success(U256::ZERO);
                SignerResponse::sign_ok(tx_hash)
            }
            Err(err) => {
                self.inner.breaker.lock().unwrap().record_failure(&err);
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                SignerResponse::error("broadcast_failed", err)
            }
        }
    }

    async fn handle_sweep_stealth(
        &self,
        chain_id: u64,
        stealth_address: Address,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        let pending_path = self.inner.store.join("stealth-pending.json");
        if !pending_path.is_file() {
            return SignerResponse::error(
                "unknown_stealth_note",
                format!(
                    "stealth address {stealth_address:#x} was not minted by propose_stealth_send for this profile"
                ),
            );
        }

        let raw = match std::fs::read_to_string(&pending_path) {
            Ok(s) => s,
            Err(e) => {
                return SignerResponse::error(
                    "unknown_stealth_note",
                    format!("could not read stealth-pending.json: {e}"),
                );
            }
        };

        let addr_str = format!("{stealth_address:#x}");
        let val: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return SignerResponse::error(
                    "unknown_stealth_note",
                    format!("could not parse stealth-pending.json: {e}"),
                );
            }
        };

        let mut found = false;
        let mut note_amount: Option<U256> = None;

        if let Some(pending_arr) = val.get("pending").and_then(|p| p.as_array()) {
            for item in pending_arr {
                if let Some(s) = item.as_str() {
                    if s.eq_ignore_ascii_case(&addr_str) {
                        found = true;
                        break;
                    }
                }
            }
        }
        if !found {
            if let Some(notes_arr) = val.get("notes").and_then(|n| n.as_array()) {
                for item in notes_arr {
                    let matches_addr = item
                        .get("stealth_address")
                        .and_then(|s| s.as_str())
                        .map(|s| s.eq_ignore_ascii_case(&addr_str))
                        .unwrap_or(false);
                    if matches_addr {
                        found = true;
                        if let Some(amt_str) = item.get("amount").and_then(|a| a.as_str()) {
                            if let Ok(amt) = std::str::FromStr::from_str(amt_str) {
                                note_amount = Some(amt);
                            }
                        }
                        break;
                    }
                }
            }
        }

        if !found {
            return SignerResponse::error(
                "unknown_stealth_note",
                format!(
                    "stealth address {stealth_address:#x} was not minted by propose_stealth_send for this profile"
                ),
            );
        }

        let balance_override = self
            .inner
            .balance_overrides
            .lock()
            .unwrap()
            .get(&Some(stealth_address))
            .copied();

        let amount = if let Some(amt) = note_amount {
            amt
        } else if let Some(b) = balance_override {
            b
        } else {
            match self.get_onchain_balance(stealth_address).await {
                Ok(b) if !b.is_zero() => b,
                _ => {
                    return SignerResponse::error(
                        "amount_undetermined",
                        format!(
                            "could not determine balance for stealth address {stealth_address:#x}"
                        ),
                    );
                }
            }
        };

        if amount.is_zero() {
            return SignerResponse::error(
                "amount_undetermined",
                format!("stealth note {stealth_address:#x} has zero balance — refusing sweep"),
            );
        }

        let asset = format!("{chain_id}:native");
        let intent = SignerIntent::SweepStealth {
            chain_id,
            stealth_address,
        };
        let key = intent.idempotency_key();
        let balance = self.get_balance(None).await.max(amount);

        // Size the reservation as full-budget (cap_pct = 100) per existing sweep rule
        match SpendLedger::reserve(
            &self.inner.store,
            &key,
            &asset,
            amount,
            balance,
            100,
            policy.enforcement,
        ) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                let code = if msg.contains("spend_already_reserved") {
                    "spend_already_reserved"
                } else {
                    "spend_guard"
                };
                return SignerResponse::error(code, msg);
            }
        }

        match self
            .inner
            .broadcaster
            .sweep_stealth(chain_id, stealth_address)
            .await
        {
            Ok(tx_hash) => {
                let _ = self
                    .inner
                    .breaker
                    .lock()
                    .unwrap()
                    .record_success(U256::ZERO);
                SignerResponse::sign_ok(tx_hash)
            }
            Err(err) => {
                self.inner.breaker.lock().unwrap().record_failure(&err);
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, &key);
                SignerResponse::error("broadcast_failed", err)
            }
        }
    }
}

/// Run the daemon loop serving Unix domain socket connections.
pub async fn run_signerd(
    store: PathBuf,
    socket: PathBuf,
    allow_uid: Vec<u32>,
    password: SecretString,
    old_profile_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    check_legacy_policy_absent(&store, old_profile_dir.as_deref())?;

    #[cfg(unix)]
    let allow_uid = if allow_uid.is_empty() {
        vec![rustix::process::getuid().as_raw()]
    } else {
        allow_uid
    };

    let wallet_path = store.join("wallet.json");
    if !wallet_path.is_file() {
        anyhow::bail!(
            "no wallet at {} — create or migrate the wallet into the store first",
            wallet_path.display()
        );
    }

    let mut wallet = WalletState::load_with_session(
        wallet_path.clone(),
        OperatingMode::SentientTrader,
        "sentient",
    )?;
    wallet.unlock(&password)?;
    let active_address = wallet
        .active_address()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .parse::<Address>()?;
    let chain_id = wallet.networks().active().chain_id;
    let (rpc_url, _) = wallet.rpc_endpoints_for(wallet.networks().active());

    #[cfg(unix)]
    {
        if let Some(parent) = socket.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if socket.exists() {
            let _ = std::fs::remove_file(&socket);
        }
    }

    let listener = tokio::net::UnixListener::bind(&socket)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660)) {
            tracing::warn!("failed to set socket mode 0660: {e}");
        }
        if let Some(gid) = find_vaughan_gid() {
            if let Err(e) = std::os::unix::fs::chown(&socket, None, Some(gid)) {
                tracing::info!(
                    "could not chgrp socket {} to vaughan (gid {gid}): {e}; continuing with current group",
                    socket.display()
                );
            }
        }
    }

    let wallet_arc = Arc::new(tokio::sync::Mutex::new(wallet));
    let broadcaster = Arc::new(broadcaster::WalletBroadcaster::new(wallet_arc));
    let quoter = Arc::new(quoter::LiveSwapQuoter);
    let signer = Signer::new(
        store,
        active_address,
        chain_id,
        rpc_url,
        quoter,
        broadcaster,
    );

    eprintln!(
        "vaughan signerd: active_address={active_address:#x} chain_id={chain_id} socket={} allow_uid={allow_uid:?}",
        socket.display()
    );

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("accept error: {e}");
                continue;
            }
        };

        let signer = signer.clone();
        let allowed = allow_uid.clone();

        tokio::spawn(async move {
            #[cfg(unix)]
            let peer_uid = match stream.peer_cred() {
                Ok(cred) => {
                    let uid = cred.uid();
                    if !is_peer_uid_allowed(uid, &allowed) {
                        tracing::warn!("peer UID {uid} rejected by allowlist {allowed:?}");
                        let resp = serde_json::to_string(&SignerResponse::error(
                            "unauthorized",
                            "peer UID not permitted",
                        ))
                        .unwrap_or_default()
                            + "\n";
                        let _ = stream.write_all(resp.as_bytes()).await;
                        return;
                    }
                    uid
                }
                Err(e) => {
                    tracing::warn!("could not read SO_PEERCRED: {e}");
                    let resp = serde_json::to_string(&SignerResponse::error(
                        "unauthorized",
                        "could not verify peer credentials",
                    ))
                    .unwrap_or_default()
                        + "\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                    return;
                }
            };
            #[cfg(not(unix))]
            let peer_uid: u32 = 0;

            let mut reader = tokio::io::BufReader::new(&mut stream);
            let mut line = String::new();
            use tokio::io::AsyncBufReadExt;
            match reader.read_line(&mut line).await {
                Ok(0) => return,
                Ok(n) if n > 65536 => {
                    line.zeroize();
                    let resp = serde_json::to_string(&SignerResponse::error(
                        "line_too_long",
                        "request line exceeded 64 KiB limit",
                    ))
                    .unwrap_or_default()
                        + "\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    line.zeroize();
                    tracing::error!("socket read error: {e}");
                    return;
                }
            }

            let response_str = signer.handle_line_with_uid(&line, peer_uid).await + "\n";
            line.zeroize();
            let _ = stream.write_all(response_str.as_bytes()).await;
        });
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
