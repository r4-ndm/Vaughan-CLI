//! Batched native transfer intent handler (`handle_batch_transfer`).
//!
//! Single purpose: per-leg destination allowlist checks, summed spend
//! reservation, and batch calldata broadcast. Must NOT contain swap, single
//! transfer, wrap, DCA, stealth sweep, or policy mutation logic.
use alloy::primitives::U256;
use vaughan_agent::sentient::SpendLedger;
use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::SignerIntent;

use crate::signerd::{balance_or_refuse, intents, Signer};

impl Signer {
    pub(crate) async fn handle_batch_transfer(
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
        let balance = balance_or_refuse!(self, None);
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

        self.finish_metered(tx, &key).await
    }
}
