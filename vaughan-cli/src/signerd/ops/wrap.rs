//! Native token wrap and unwrap intent handlers (`handle_wrap`, `handle_unwrap`).
//!
//! Single purpose: wrapped-native deposit/withdraw spend reservation, calldata
//! construction, and broadcast. Must NOT contain transfer, swap, approval, DCA,
//! stealth sweep, or policy mutation logic.
use alloy::primitives::U256;
use vaughan_agent::sentient::SpendLedger;
use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::SignerIntent;

use crate::signerd::{balance_or_refuse, intents, Signer};

impl Signer {
    pub(crate) async fn handle_wrap(
        &self,
        chain_id: u64,
        amount: U256,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        let asset = format!("{chain_id}:native");
        let balance = balance_or_refuse!(self, None);
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

    pub(crate) async fn handle_unwrap(
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
        let balance = balance_or_refuse!(self, Some(weth));
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
}
