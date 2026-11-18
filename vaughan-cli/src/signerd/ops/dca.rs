//! DCA slice swap intent handler (`handle_dca_slice`).
//!
//! Single purpose: live pair quote verification, corridor floor check, spend
//! reservation, and DCA slice swap broadcast. Must NOT contain standalone swap,
//! transfer, wrap, stealth sweep, or policy mutation logic.
use alloy::primitives::{Address, U256};
use vaughan_agent::sentient::SpendLedger;
use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::SignerIntent;

use crate::signerd::{balance_or_refuse, intents, swap_input_or_refuse, Signer};

impl Signer {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle_dca_slice(
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

        let (balance_token, asset) = swap_input_or_refuse!(chain_id, dca_quote.token_in);
        let balance = balance_or_refuse!(self, balance_token);
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

        self.finish_metered(tx, &key).await
    }
}
