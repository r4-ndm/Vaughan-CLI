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
pub mod quoter;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alloy::primitives::{Address, U256};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use vaughan_agent::sentient::policy::EnforcementMode;
use vaughan_agent::sentient::SpendLedger;
use vaughan_core::core::{OperatingMode, SignerIntent, SwapFamily, WalletState};

pub use broadcaster::{MockBroadcaster, TransactionBroadcaster, WalletBroadcaster};
pub use quoter::{LiveSwapQuoter, MockSwapQuoter, SwapQuoter};

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

/// Request sent over the Unix socket.
#[derive(Debug, Clone, Deserialize)]
pub struct SignerRequest {
    #[serde(default)]
    pub v: Option<u32>,
    pub op: String,
    #[serde(default)]
    pub intent: Option<SignerIntent>,
}

/// Response returned by the signer daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignerResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl SignerResponse {
    pub fn status(address: Address, chain_id: u64, store: String, policy_digest: String) -> Self {
        Self {
            ok: true,
            address: Some(format!("{address:#x}")),
            chain_id: Some(chain_id),
            store: Some(store),
            policy_digest: Some(policy_digest),
            tx_hash: None,
            code: None,
            message: None,
        }
    }

    pub fn sign_ok(tx_hash: impl Into<String>) -> Self {
        Self {
            ok: true,
            address: None,
            chain_id: None,
            store: None,
            policy_digest: None,
            tx_hash: Some(tx_hash.into()),
            code: None,
            message: None,
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            address: None,
            chain_id: None,
            store: None,
            policy_digest: None,
            tx_hash: None,
            code: Some(code.into()),
            message: Some(message.into()),
        }
    }
}

struct SignerInner {
    store: PathBuf,
    active_address: Address,
    chain_id: u64,
    rpc_url: String,
    quoter: Arc<dyn SwapQuoter>,
    broadcaster: Arc<dyn TransactionBroadcaster>,
    balance_overrides: Mutex<HashMap<Option<Address>, U256>>,
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
        Self {
            inner: Arc::new(SignerInner {
                store,
                active_address,
                chain_id,
                rpc_url,
                quoter,
                broadcaster,
                balance_overrides: Mutex::new(HashMap::new()),
            }),
        }
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

    /// Handle a newline-delimited JSON line and return the JSON response string.
    pub async fn handle_line(&self, line: &str) -> String {
        let resp = self.handle_line_to_response(line).await;
        serde_json::to_string(&resp).unwrap_or_else(|e| {
            format!(r#"{{"ok":false,"code":"internal_error","message":"{e}"}}"#)
        })
    }

    /// Parse and dispatch a single JSON line to `SignerResponse`.
    pub async fn handle_line_to_response(&self, line: &str) -> SignerResponse {
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
        SignerResponse::status(
            self.inner.active_address,
            self.inner.chain_id,
            self.inner.store.display().to_string(),
            policy.digest(),
        )
    }

    /// Handle `{"op":"sign","intent":{…}}`.
    pub async fn handle_sign(&self, intent: SignerIntent) -> SignerResponse {
        // 1. Specific refusal for declared but deferred intents (S2)
        match &intent {
            SignerIntent::DcaSlice { .. } => {
                return SignerResponse::error(
                    "intent_not_implemented",
                    "dca_slice is not implemented in S1 (scheduled for S2)",
                );
            }
            SignerIntent::SweepStealth { .. } => {
                return SignerResponse::error(
                    "intent_not_implemented",
                    "sweep_stealth is not implemented in S1 (scheduled for S2)",
                );
            }
            _ => {}
        }

        // 2. Chain ID match
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

        // 3. Load active policy
        let policy = match vaughan_agent::sentient::load_policy(&self.inner.store) {
            Ok(p) => p,
            Err(e) => {
                return SignerResponse::error(
                    "policy_load_failed",
                    format!("could not load policy: {e}"),
                );
            }
        };

        // 4. Dispatch by intent variant
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

            SignerIntent::DcaSlice { .. } | SignerIntent::SweepStealth { .. } => unreachable!(),
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
            Ok(tx_hash) => SignerResponse::sign_ok(tx_hash),
            Err(err) => {
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

        match self.inner.broadcaster.broadcast(tx).await {
            Ok(tx_hash) => SignerResponse::sign_ok(tx_hash),
            Err(err) => {
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
            Ok(tx_hash) => SignerResponse::sign_ok(tx_hash),
            Err(err) => {
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
            Ok(tx_hash) => SignerResponse::sign_ok(tx_hash),
            Err(err) => {
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
            Ok(tx_hash) => SignerResponse::sign_ok(tx_hash),
            Err(err) => {
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
            Ok(tx_hash) => SignerResponse::sign_ok(tx_hash),
            Err(err) => {
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
) -> anyhow::Result<()> {
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
            {
                match stream.peer_cred() {
                    Ok(cred) => {
                        let peer_uid = cred.uid();
                        if !is_peer_uid_allowed(peer_uid, &allowed) {
                            tracing::warn!("peer UID {peer_uid} rejected by allowlist {allowed:?}");
                            let resp = serde_json::to_string(&SignerResponse::error(
                                "unauthorized",
                                "peer UID not permitted",
                            ))
                            .unwrap_or_default()
                                + "\n";
                            let _ = stream.write_all(resp.as_bytes()).await;
                            return;
                        }
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
                }
            }

            let mut reader = tokio::io::BufReader::new(&mut stream);
            let mut line = String::new();
            use tokio::io::AsyncBufReadExt;
            match reader.read_line(&mut line).await {
                Ok(0) => return,
                Ok(n) if n > 65536 => {
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
                    tracing::error!("socket read error: {e}");
                    return;
                }
            }

            let response_str = signer.handle_line(&line).await + "\n";
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
