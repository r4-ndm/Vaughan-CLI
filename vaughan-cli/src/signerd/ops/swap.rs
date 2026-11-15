//! DEX swap intent handler (`handle_swap`).
//!
//! Single purpose: router allowlist, live re-quote, slippage floor enforcement,
//! spend reservation, and swap broadcast. Must NOT contain DCA state handling,
//! transfer, wrap, stealth sweep, or policy mutation logic.
use alloy::primitives::{Address, U256};
use vaughan_agent::sentient::SpendLedger;
use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::{SignerIntent, SwapFamily};

use crate::signerd::{intents, Signer};

impl Signer {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle_swap(
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
}
