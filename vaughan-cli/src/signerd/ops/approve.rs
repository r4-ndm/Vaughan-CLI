//! ERC-20 token approval intent handler (`handle_approve`).
//!
//! Single purpose: max-approval refusal, spender allowlist, expiry check, cap
//! enforcement, spend reservation, and approve broadcast. Must NOT contain swap,
//! transfer, wrap, DCA, stealth sweep, or policy mutation logic.
use alloy::primitives::{Address, U256};
use vaughan_agent::sentient::policy::EnforcementMode;
use vaughan_agent::sentient::SpendLedger;
use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::SignerIntent;

use crate::signerd::{balance_or_refuse, intents, Signer};

impl Signer {
    pub(crate) async fn handle_approve(
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
        let balance = balance_or_refuse!(self, Some(token));
        let cap_pct = policy.max_approve_pct();
        let cap = balance.saturating_mul(U256::from(cap_pct)) / U256::from(100);
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

        self.finish_metered(tx, &key).await
    }
}
