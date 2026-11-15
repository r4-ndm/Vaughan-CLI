//! Policy inspection, prechecks, and root-approved policy update handlers.
//!
//! Single purpose: `Signer` policy query (`status`, `policy`), tighten-only
//! classification, and `policy_update` (password, stage, approved) handlers.
//! Must NOT contain transaction signing, calldata building, or socket server
//! accept-loop logic.

use std::time::Instant;

use secrecy::{ExposeSecret, SecretString};
use vaughan_agent::sentient::policy::EnforcementMode;
use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::{OperatingMode, WalletState};

use super::{approval, Signer, STEP_UP_MAX_ATTEMPTS, STEP_UP_WINDOW};

impl Signer {
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

    fn is_request_tighten_only(&self, policy_val: Option<&serde_json::Value>) -> bool {
        let Some(val) = policy_val else {
            return false;
        };
        let Ok(candidate) =
            serde_json::from_value::<vaughan_agent::sentient::AgentSessionPolicy>(val.clone())
        else {
            return false;
        };
        let Ok(current) = vaughan_agent::sentient::load_policy(&self.inner.store) else {
            return false;
        };
        current.is_tighten_only(&candidate)
    }

    fn precheck_policy_update(
        &self,
        policy_val: Option<serde_json::Value>,
        expected_digest: Option<String>,
    ) -> Result<
        (
            vaughan_agent::sentient::AgentSessionPolicy,
            vaughan_agent::sentient::AgentSessionPolicy,
        ),
        Box<SignerResponse>,
    > {
        let old_policy = match vaughan_agent::sentient::load_policy(&self.inner.store) {
            Ok(p) => p,
            Err(e) => {
                return Err(Box::new(SignerResponse::error(
                    "policy_load_failed",
                    format!("could not load current policy: {e}"),
                )));
            }
        };

        if let Some(expected) = expected_digest {
            let current_digest = old_policy.digest();
            if expected != current_digest {
                return Err(Box::new(SignerResponse::error(
                    "policy_conflict",
                    format!(
                        "policy digest mismatch: expected {expected}, current is {current_digest}"
                    ),
                )));
            }
        }

        let Some(val) = policy_val else {
            return Err(Box::new(SignerResponse::error(
                "policy_invalid",
                "missing 'policy' object",
            )));
        };

        let new_policy: vaughan_agent::sentient::AgentSessionPolicy =
            match serde_json::from_value(val) {
                Ok(p) => p,
                Err(e) => {
                    return Err(Box::new(SignerResponse::error(
                        "policy_invalid",
                        format!("invalid policy payload: {e}"),
                    )));
                }
            };

        if new_policy.enforcement == EnforcementMode::Disabled
            && (!old_policy.acknowledge_unsafe || !new_policy.acknowledge_unsafe)
        {
            return Err(Box::new(SignerResponse::error(
                "policy_refused",
                "enforcement=disabled requires acknowledge_unsafe=true already persisted on disk (two-step confirm-unsafe required)",
            )));
        }

        if let Err(e) = new_policy.validate() {
            return Err(Box::new(SignerResponse::error(
                "policy_invalid",
                e.to_string(),
            )));
        }

        Ok((old_policy, new_policy))
    }

    fn apply_validated_policy_update(
        &self,
        old_policy: &vaughan_agent::sentient::AgentSessionPolicy,
        new_policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        if let Err(e) = vaughan_agent::sentient::save_policy(&self.inner.store, new_policy) {
            return SignerResponse::error("policy_save_failed", e.to_string());
        }

        if let Err(e) =
            vaughan_agent::sentient::append_policy_audit(&self.inner.store, old_policy, new_policy)
        {
            let _ = vaughan_agent::sentient::save_policy(&self.inner.store, old_policy);
            return SignerResponse::error("policy_audit_failed", e.to_string());
        }

        if let Ok(cfg) = vaughan_agent::breaker_config_for_session(Some(&self.inner.store), 1) {
            self.inner.breaker.lock().unwrap().replace_config(cfg);
        }

        SignerResponse::policy_update_ok(new_policy.digest())
    }

    fn validate_and_apply_policy_update(
        &self,
        policy_val: Option<serde_json::Value>,
        expected_digest: Option<String>,
    ) -> SignerResponse {
        let (old_policy, new_policy) =
            match self.precheck_policy_update(policy_val, expected_digest) {
                Ok(pair) => pair,
                Err(resp) => return *resp,
            };
        self.apply_validated_policy_update(&old_policy, &new_policy)
    }

    /// Handle `{"op":"policy_update","password":"…","policy":{…}}`.
    ///
    /// Rules:
    /// 1. Verify vault password via `spawn_blocking` KDF before touching or accepting any policy.
    /// 2. Rate-limit step-up attempts per peer UID (5 attempts per 60s window; tighten-only updates exempt).
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

        let tighten_only = self.is_request_tighten_only(policy_val.as_ref());

        // Per-peer-UID backoff before running Argon2id in spawn_blocking (tighten-only exempt).
        if !tighten_only {
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
                if !tighten_only {
                    let mut limiter = self.inner.step_up_limiter.lock().unwrap();
                    if let Some(entries) = limiter.get_mut(&peer_uid) {
                        entries.pop();
                    }
                }
            }
            _ => {
                return SignerResponse::error(
                    "step_up_failed",
                    "vault password verification failed",
                );
            }
        }

        self.validate_and_apply_policy_update(policy_val, expected_digest)
    }

    /// Handle `{"op":"policy_update","auth":"stage","policy":{…},"expected_digest":"…"}`.
    pub fn handle_policy_stage(
        &self,
        policy_val: Option<serde_json::Value>,
        expected_digest: Option<String>,
    ) -> SignerResponse {
        let (old_policy, new_policy) =
            match self.precheck_policy_update(policy_val, expected_digest) {
                Ok(pair) => pair,
                Err(resp) => return *resp,
            };

        let allow_replace = old_policy.is_tighten_only(&new_policy);
        let now = vaughan_agent::dca::now_unix();
        match approval::stage_pending_policy_with_replace(
            &self.inner.store,
            &new_policy,
            now,
            allow_replace,
        ) {
            Ok(()) => SignerResponse::step_up_required(new_policy.digest()),
            Err(approval::PendingStageError::AlreadyPending) => SignerResponse::error(
                "step_up_pending",
                "a staged policy update is already pending approval",
            ),
            Err(approval::PendingStageError::Io(msg)) => {
                SignerResponse::error("policy_save_failed", msg)
            }
        }
    }

    /// Handle `{"op":"policy_update","auth":"approved","policy":{…}}`.
    pub fn handle_policy_approved(
        &self,
        policy_val: Option<serde_json::Value>,
        expected_digest: Option<String>,
        peer_uid: u32,
    ) -> SignerResponse {
        let tighten_only = self.is_request_tighten_only(policy_val.as_ref());
        if !tighten_only {
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

        // (1) Parse request policy before touching pending or approval files.
        let Some(val) = policy_val else {
            return SignerResponse::error(
                approval::STEP_UP_FAILED_CODE,
                approval::STEP_UP_FAILED_MSG,
            );
        };
        let candidate: vaughan_agent::sentient::AgentSessionPolicy =
            match serde_json::from_value(val.clone()) {
                Ok(p) => p,
                Err(_) => {
                    return SignerResponse::error(
                        approval::STEP_UP_FAILED_CODE,
                        approval::STEP_UP_FAILED_MSG,
                    );
                }
            };

        // (2) Read staged pending policy without touching policy-approval.json.
        let now = vaughan_agent::dca::now_unix();
        let pending = match approval::load_pending_policy(&self.inner.store, now) {
            Ok(p) => p,
            Err(_) => {
                return SignerResponse::error(
                    approval::STEP_UP_FAILED_CODE,
                    approval::STEP_UP_FAILED_MSG,
                );
            }
        };

        // (3) Prove this request matches the staged pending policy before unlinking approval.
        if candidate.digest() != pending.policy.digest() {
            return SignerResponse::error(
                approval::STEP_UP_FAILED_CODE,
                approval::STEP_UP_FAILED_MSG,
            );
        }

        // (4) Only now consume (fstat-verify and unlink) policy-approval.json.
        let expected_uid = self
            .inner
            .expected_approval_uid
            .load(std::sync::atomic::Ordering::Relaxed);
        let approval = match approval::consume_approval(&self.inner.store, expected_uid, now) {
            Ok(a) => a,
            Err(_) => {
                return SignerResponse::error(
                    approval::STEP_UP_FAILED_CODE,
                    approval::STEP_UP_FAILED_MSG,
                );
            }
        };

        // (5) Bind candidate to approval.new_digest, check caller expected_digest, re-check disk & apply.
        if candidate.digest() != approval.new_digest {
            return SignerResponse::error(
                approval::STEP_UP_FAILED_CODE,
                approval::STEP_UP_FAILED_MSG,
            );
        }

        if !tighten_only {
            let mut limiter = self.inner.step_up_limiter.lock().unwrap();
            if let Some(entries) = limiter.get_mut(&peer_uid) {
                entries.pop();
            }
        }

        if let Some(ref caller_expected) = expected_digest {
            if caller_expected != &approval.old_digest {
                return SignerResponse::error(
                    "policy_conflict",
                    format!(
                        "policy digest mismatch: expected {caller_expected}, approval bound {}",
                        approval.old_digest
                    ),
                );
            }
        }

        let resp = self.validate_and_apply_policy_update(Some(val), Some(approval.old_digest));
        if resp.ok {
            approval::clear_pending_policy(&self.inner.store);
        }
        resp
    }
}
