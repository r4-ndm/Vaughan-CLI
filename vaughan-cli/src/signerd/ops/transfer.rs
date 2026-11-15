//! Native and ERC-20 transfer intent handler (`handle_transfer`).
//!
//! Single purpose: destination allowlist, position cap, spend reservation, and
//! transfer broadcast. Must NOT contain swap, approval, wrap, DCA, stealth
//! sweep, or policy mutation logic.
use alloy::primitives::{Address, U256};
use vaughan_agent::sentient::policy::EnforcementMode;
use vaughan_agent::sentient::SpendLedger;
use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::SignerIntent;

use crate::signerd::{balance_or_refuse, intents, Signer};

impl Signer {
    pub(crate) async fn handle_transfer(
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

        let balance = balance_or_refuse!(self, token);
        let cap = balance.saturating_mul(U256::from(policy.max_position_pct)) / U256::from(100);
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
}
