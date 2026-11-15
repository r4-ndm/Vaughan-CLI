//! Intent signing entrypoint and policy/circuit-breaker gate.
//!
//! Single purpose: `Signer::handle_sign` circuit-breaker check, chain-ID guard,
//! policy load, and dispatch to per-intent handlers in `ops`. Must NOT contain
//! policy mutation, calldata construction, or socket server logic.

use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::SignerIntent;

use super::Signer;

impl Signer {
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

        // 1b. Mainnet writes need the operator's opt-in
        if let Some(resp) = self.mainnet_refusal(intent.chain_id()) {
            return resp;
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
}

impl Signer {
    /// `mainnet_blocked` unless the chain is a known testnet or the operator opted in.
    /// A chain this build does not know counts as mainnet: refusing is the safe default.
    fn mainnet_refusal(&self, chain_id: u64) -> Option<SignerResponse> {
        let is_testnet = vaughan_core::chains::evm::networks::get_network_by_chain_id(chain_id)
            .is_some_and(|n| n.is_testnet);
        let allowed = self
            .inner
            .mainnet_writes_allowed
            .load(std::sync::atomic::Ordering::Relaxed);
        (!is_testnet && !allowed).then(|| {
            SignerResponse::error(
                "mainnet_blocked",
                format!(
                    "chain {chain_id} is not a known testnet; the signer daemon needs \
                     VAUGHAN_MCP_ALLOW_MAINNET=1 in its environment (/etc/vaughan/sentient.env) to sign there"
                ),
            )
        })
    }
}
