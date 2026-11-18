//! `Signer::broadcast_metered`: the one path that prices a tx, reserves its
//! worst-case fee against the session gas ceiling, re-checks a trip
//! immediately before send, and decides what happens to the reservation
//! after broadcast. No op-specific calldata, allowlist, or slippage logic
//! lives here — callers build the tx, this only prices/reserves/sends it.

use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::signer_client::SignerResponse;

use super::Signer;

/// Whether a failed send definitely never moved money, or may have (mirrors
/// [`vaughan_core::core::broadcasts::BroadcastPhase`] on this crate's side of the
/// `TransactionBroadcaster` trait).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendPhase {
    /// The node rejected the tx outright, or it was never sent. Safe to release the
    /// reservation and count it as an error.
    Definitive,
    /// The raw tx may be sitting in a mempool. Keep the reservation; do not count it as an
    /// error (an identical retry must not be re-armed — see `spend_already_reserved`).
    Uncertain,
}

/// Outcome of a metered broadcast attempt that every `ops::handle_*` turns into a
/// [`SignerResponse`] and a ledger decision.
pub(super) enum MeteredError {
    /// Nothing was sent; any gas reservation has already been released.
    Refused(Box<SignerResponse>),
    /// The broadcaster returned `Err` after a reservation was made; `phase` decides what the
    /// caller does with the ledger legs and the gas reservation (see [`Signer::finish_metered`]).
    Broadcast { phase: SendPhase, message: String },
}

impl Signer {
    /// Quote, reserve, re-check the trip, and broadcast `tx`.
    ///
    /// On success the gas reservation STAYS: the worst-case fee is booked, real `gasUsed` is
    /// unknown without waiting for a receipt, and we deliberately do not wait for one.
    ///
    /// Honest limit: the trip re-check happens immediately before send, but it cannot recall
    /// an envelope already handed to the RPC — the window is one sign+send round trip.
    pub(super) async fn broadcast_metered(
        &self,
        tx: EvmTransaction,
    ) -> Result<String, MeteredError> {
        let quote = self.inner.broadcaster.quote_fee(tx).await.map_err(|e| {
            MeteredError::Refused(Box::new(SignerResponse::error(
                "gas_unavailable",
                format!("could not price this transaction: {e}"),
            )))
        })?;

        if let Err(e) = self
            .inner
            .breaker
            .lock()
            .unwrap()
            .reserve_gas(quote.max_fee_wei)
        {
            let code = if matches!(e, vaughan_agent::AgentError::CircuitBreakerTripped(_)) {
                "circuit_breaker_tripped"
            } else {
                "gas_ceiling_exceeded"
            };
            return Err(MeteredError::Refused(Box::new(SignerResponse::error(
                code,
                e.to_string(),
            ))));
        }

        let (tripped, reason) = self.inner.breaker.lock().unwrap().trip_state();
        if tripped {
            self.inner
                .breaker
                .lock()
                .unwrap()
                .release_gas(quote.max_fee_wei);
            return Err(MeteredError::Refused(Box::new(SignerResponse::error(
                "circuit_breaker_tripped",
                format!(
                    "circuit breaker tripped: {}",
                    reason.unwrap_or_else(|| "halted".into())
                ),
            ))));
        }

        match self.inner.broadcaster.broadcast_phased(quote.tx).await {
            Ok(hash) => {
                self.inner.breaker.lock().unwrap().record_success_no_gas();
                Ok(hash)
            }
            Err((SendPhase::Definitive, message)) => {
                self.inner
                    .breaker
                    .lock()
                    .unwrap()
                    .release_gas(quote.max_fee_wei);
                self.inner.breaker.lock().unwrap().record_failure(&message);
                Err(MeteredError::Broadcast {
                    phase: SendPhase::Definitive,
                    message,
                })
            }
            Err((SendPhase::Uncertain, message)) => Err(MeteredError::Broadcast {
                phase: SendPhase::Uncertain,
                message,
            }),
        }
    }

    /// Turn a [`Self::broadcast_metered`] outcome into the `SignerResponse` + ledger decision
    /// every op handler needs, so the per-op bodies in `ops::*` do not each repeat it: release
    /// the spend-ledger legs for `key` on refusal or a definitive failure (no money moved);
    /// keep them on success or an uncertain failure (money may have moved).
    pub(super) async fn finish_metered(&self, tx: EvmTransaction, key: &str) -> SignerResponse {
        match self.broadcast_metered(tx).await {
            Ok(hash) => SignerResponse::sign_ok(hash),
            Err(MeteredError::Refused(resp)) => {
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, key);
                *resp
            }
            Err(MeteredError::Broadcast {
                phase: SendPhase::Definitive,
                message,
            }) => {
                let _ = vaughan_agent::sentient::release_legs(&self.inner.store, key);
                SignerResponse::error("broadcast_failed", message)
            }
            Err(MeteredError::Broadcast {
                phase: SendPhase::Uncertain,
                message,
            }) => SignerResponse::error("broadcast_uncertain", message),
        }
    }
}
