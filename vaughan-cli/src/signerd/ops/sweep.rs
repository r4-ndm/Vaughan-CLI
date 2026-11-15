//! Stealth address sweep intent handler (`handle_sweep_stealth`).
//!
//! Single purpose: `stealth-pending.json` ownership lookup, balance resolution,
//! full-budget spend reservation, and sweep broadcast. Must NOT contain standard
//! transfer, swap, wrap, DCA, or policy mutation logic.
use alloy::primitives::{Address, U256};
use vaughan_agent::sentient::SpendLedger;
use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::SignerIntent;

use crate::signerd::Signer;

/// Budget of the stealth-sweep ledger bucket: unlimited, see `handle_sweep_stealth`.
///
/// The ledger `amount` of a sweep is bookkeeping only: the broadcaster sweeps whatever the
/// wallet's own note scan finds at the address, so the recorded amount must not be read as
/// the quantity actually moved.
const SWEEP_BUDGET: U256 = U256::MAX;

impl Signer {
    pub(crate) async fn handle_sweep_stealth(
        &self,
        chain_id: u64,
        stealth_address: Address,
        policy: &vaughan_agent::sentient::AgentSessionPolicy,
    ) -> SignerResponse {
        let pending_path = self.inner.store.join("stealth-pending.json");
        if !pending_path.is_file() {
            return SignerResponse::error(
                "unknown_stealth_note",
                format!(
                    "stealth address {stealth_address:#x} was not minted by propose_stealth_send for this profile"
                ),
            );
        }

        let raw = match std::fs::read_to_string(&pending_path) {
            Ok(s) => s,
            Err(e) => {
                return SignerResponse::error(
                    "unknown_stealth_note",
                    format!("could not read stealth-pending.json: {e}"),
                );
            }
        };

        let addr_str = format!("{stealth_address:#x}");
        let val: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return SignerResponse::error(
                    "unknown_stealth_note",
                    format!("could not parse stealth-pending.json: {e}"),
                );
            }
        };

        let mut found = false;
        let mut note_amount: Option<U256> = None;

        if let Some(pending_arr) = val.get("pending").and_then(|p| p.as_array()) {
            for item in pending_arr {
                if let Some(s) = item.as_str() {
                    if s.eq_ignore_ascii_case(&addr_str) {
                        found = true;
                        break;
                    }
                }
            }
        }
        if !found {
            if let Some(notes_arr) = val.get("notes").and_then(|n| n.as_array()) {
                for item in notes_arr {
                    let matches_addr = item
                        .get("stealth_address")
                        .and_then(|s| s.as_str())
                        .map(|s| s.eq_ignore_ascii_case(&addr_str))
                        .unwrap_or(false);
                    if matches_addr {
                        found = true;
                        if let Some(amt_str) = item.get("amount").and_then(|a| a.as_str()) {
                            if let Ok(amt) = std::str::FromStr::from_str(amt_str) {
                                note_amount = Some(amt);
                            }
                        }
                        break;
                    }
                }
            }
        }

        if !found {
            return SignerResponse::error(
                "unknown_stealth_note",
                format!(
                    "stealth address {stealth_address:#x} was not minted by propose_stealth_send for this profile"
                ),
            );
        }

        let amount = match self.sweep_amount(stealth_address, note_amount).await {
            Ok(a) => a,
            Err(resp) => return resp,
        };

        if amount.is_zero() {
            return SignerResponse::error(
                "amount_undetermined",
                format!("stealth note {stealth_address:#x} has zero balance — refusing sweep"),
            );
        }

        // A sweep moves funds the profile already owns INTO the active wallet, so it neither
        // consumes nor sizes the active wallet's outgoing native allowance. It gets its own
        // bucket with no spending limit (`SWEEP_BUDGET`): the reservation exists for replay
        // protection (idempotency), and several notes may be swept in one window.
        let asset = format!("{chain_id}:stealth-sweep");
        let intent = SignerIntent::SweepStealth {
            chain_id,
            stealth_address,
        };
        let key = intent.idempotency_key();

        match SpendLedger::reserve(
            &self.inner.store,
            &key,
            &asset,
            amount,
            SWEEP_BUDGET,
            100,
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

        match self
            .inner
            .broadcaster
            .sweep_stealth(chain_id, stealth_address)
            .await
        {
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

impl Signer {
    /// The amount a sweep is accounted at: the pending note's recorded amount, else the
    /// stealth address's live balance. An unreadable balance is `balance_unavailable`
    /// (never a guess); an empty one is `amount_undetermined`.
    async fn sweep_amount(
        &self,
        stealth_address: Address,
        note_amount: Option<U256>,
    ) -> Result<U256, SignerResponse> {
        if let Some(amt) = note_amount {
            return Ok(amt);
        }
        match self
            .inner
            .balances
            .balance_of(&self.inner.rpc_url, stealth_address, None)
            .await
        {
            Ok(b) if !b.is_zero() => Ok(b),
            Ok(_) => Err(SignerResponse::error(
                "amount_undetermined",
                format!("stealth address {stealth_address:#x} holds no funds to sweep"),
            )),
            Err(e) => Err(SignerResponse::error(
                "balance_unavailable",
                format!("could not read balance of stealth address {stealth_address:#x}: {e}"),
            )),
        }
    }
}
