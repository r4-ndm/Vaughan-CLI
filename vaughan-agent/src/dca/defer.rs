//! One rule in one place: which refused slice is DEFERRED rather than FAILED, and
//! how each kind is written back into the plan.
//!
//! A refusal that moved no money (the signer daemon's gas ceiling, an unpriceable
//! fee, a tripped breaker) is not the plan's fault: counting it would pause a healthy
//! plan after three refusals (`MAX_CONSECUTIVE_FAILURES`) and each one would burn a
//! buy window. A refusal that may have moved money (`broadcast_uncertain`) is a
//! failure and must stay one.
//!
//! Must NOT contain: signer IPC, quote/route logic, trigger math, or store I/O.

use alloy::primitives::U256;

use super::trigger::advance_schedule;
use super::types::{DcaPlan, DcaStatus, SliceLog, MAX_CONSECUTIVE_FAILURES, MAX_SLICE_LOG};
use crate::error::AgentError;
use crate::sentient::{GAS_CEILING_REFUSAL_PREFIX, TRADING_HALTED_PREFIX};

/// How soon a deferred slice is re-tried, capped by the plan's own interval.
///
/// The window is NOT advanced (that is the point of deferring), but re-asking the
/// daemon on every UI tick would hammer the socket the whole time the ceiling is
/// exhausted — `RetryClass::ShrinkOrWait` says explicitly not to.
const DEFER_RETRY_SECS: u64 = 60;

/// Whether a refused slice keeps its place in the schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceRefusal {
    /// Nothing was sent and nothing was spent: no failure count, no window burned.
    Deferred,
    /// Counts against the plan (may have executed, or the plan itself is wrong).
    Failed,
}

/// Classify an error from the in-process [`crate::sentient::SentientTrader`] path.
///
/// Mirrors the signer-daemon classifier in `vaughan-tui::dca_defer`, which sees the
/// same refusals as `{code}: {message}` strings over IPC. Deliberately NOT deferred:
/// the trader's retrospective ceiling trip (`record_success`) fires *after* the
/// broadcast, so that money has already moved and the slice must count.
pub fn classify_agent_error(err: &AgentError) -> SliceRefusal {
    match err {
        // Pre-flight halt: the breaker refused before anything was signed.
        AgentError::CircuitBreakerTripped(m) if m.starts_with(TRADING_HALTED_PREFIX) => {
            SliceRefusal::Deferred
        }
        // Gas reservation refused by the session ceiling; nothing was sent.
        AgentError::InvalidToolCall(m) if m.starts_with(GAS_CEILING_REFUSAL_PREFIX) => {
            SliceRefusal::Deferred
        }
        _ => SliceRefusal::Failed,
    }
}

/// Write a refused slice back into `plan`.
///
/// `Deferred` leaves `consecutive_failures`, `status` and the buy window alone (only
/// nudging `next_due_at` by [`DEFER_RETRY_SECS`] so the retry is not a tight loop);
/// `Failed` keeps the original behaviour (count, pause at the limit, advance).
pub fn apply_refusal(
    plan: &mut DcaPlan,
    now: u64,
    class: SliceRefusal,
    reason: &str,
    amount_in: &U256,
) {
    match class {
        SliceRefusal::Deferred => {
            let interval = plan
                .trigger
                .interval_secs()
                .unwrap_or(DEFER_RETRY_SECS)
                .max(1);
            plan.next_due_at = now.saturating_add(DEFER_RETRY_SECS.min(interval));
        }
        SliceRefusal::Failed => {
            plan.consecutive_failures = plan.consecutive_failures.saturating_add(1);
            if plan.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                plan.status = DcaStatus::Paused;
            }
            // Still advance so we do not hammer the same second on transient errors.
            advance_schedule(plan, now);
        }
    }
    let error = match class {
        SliceRefusal::Deferred => format!("deferred (not counted): {reason}"),
        SliceRefusal::Failed => reason.to_string(),
    };
    push_log(
        plan,
        SliceLog {
            at: now,
            ok: false,
            tx_hash: String::new(),
            dry_run: plan.dry_run,
            error,
            amount_in_wei: amount_in.to_string(),
        },
    );
}

/// Append one slice-log entry, keeping the log bounded.
pub(super) fn push_log(plan: &mut DcaPlan, entry: SliceLog) {
    plan.slice_log.push(entry);
    if plan.slice_log.len() > MAX_SLICE_LOG {
        let drop_n = plan.slice_log.len() - MAX_SLICE_LOG;
        plan.slice_log.drain(0..drop_n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dca::types::{DcaTrigger, DcaVenue};

    fn plan() -> DcaPlan {
        DcaPlan {
            id: "p".into(),
            chain_id: 369,
            account: "0x1111111111111111111111111111111111111111".into(),
            token_out: "0x2222222222222222222222222222222222222222".into(),
            slice_amount_wei: "1".into(),
            trigger: DcaTrigger::Time {
                interval_secs: 3600,
            },
            venue: DcaVenue::default(),
            max_slippage_bps: 100,
            max_slices: 5,
            slices_done: 0,
            spent_wei: "0".into(),
            consecutive_failures: 0,
            status: DcaStatus::Active,
            created_at: 0,
            next_due_at: 1_000,
            dry_run: false,
            slice_log: vec![],
            pair: None,
        }
    }

    #[test]
    fn gas_refusals_from_the_in_process_trader_are_deferred() {
        assert_eq!(
            classify_agent_error(&AgentError::CircuitBreakerTripped(format!(
                "{TRADING_HALTED_PREFIX}kill switch: Ctrl+K"
            ))),
            SliceRefusal::Deferred
        );
        assert_eq!(
            classify_agent_error(&AgentError::InvalidToolCall(format!(
                "{GAS_CEILING_REFUSAL_PREFIX} 5 wei would be exceeded: spent 4 wei"
            ))),
            SliceRefusal::Deferred
        );
    }

    #[test]
    fn other_errors_and_the_post_broadcast_trip_still_count() {
        // Retrospective ceiling trip: the swap was already broadcast.
        assert_eq!(
            classify_agent_error(&AgentError::CircuitBreakerTripped(
                "Trading session gas ceiling exceeded".into()
            )),
            SliceRefusal::Failed
        );
        assert_eq!(
            classify_agent_error(&AgentError::InvalidToolCall(
                "insufficient funds for swap value + gas".into()
            )),
            SliceRefusal::Failed
        );
        assert_eq!(
            classify_agent_error(&AgentError::ProviderError("rpc down".into())),
            SliceRefusal::Failed
        );
        assert_eq!(
            classify_agent_error(&AgentError::RpcQuorumMismatch),
            SliceRefusal::Failed
        );
    }

    #[test]
    fn deferred_refusal_keeps_the_window_and_the_failure_count() {
        let mut p = plan();
        apply_refusal(
            &mut p,
            2_000,
            SliceRefusal::Deferred,
            "gas_ceiling_exceeded: no room",
            &U256::from(7),
        );

        assert_eq!(p.consecutive_failures, 0);
        assert_eq!(p.status, DcaStatus::Active);
        // Re-checked soon (60 s), not pushed out by the 3600 s interval.
        assert_eq!(p.next_due_at, 2_060);
        assert_eq!(p.slice_log.len(), 1);
        assert!(!p.slice_log[0].ok);
        assert!(p.slice_log[0].error.contains("deferred (not counted)"));
        assert!(p.slice_log[0].error.contains("gas_ceiling_exceeded"));
        assert_eq!(p.slice_log[0].amount_in_wei, "7");
    }

    #[test]
    fn deferred_retry_never_exceeds_the_plan_interval() {
        let mut p = plan();
        p.trigger = DcaTrigger::Time { interval_secs: 10 };
        apply_refusal(
            &mut p,
            2_000,
            SliceRefusal::Deferred,
            "gas_unavailable: rpc down",
            &U256::ZERO,
        );
        assert_eq!(p.next_due_at, 2_010);
    }

    #[test]
    fn failed_refusal_counts_pauses_at_the_limit_and_advances() {
        let mut p = plan();
        for i in 1..=MAX_CONSECUTIVE_FAILURES {
            apply_refusal(
                &mut p,
                2_000,
                SliceRefusal::Failed,
                "broadcast_uncertain: timeout",
                &U256::ZERO,
            );
            assert_eq!(p.consecutive_failures, i);
        }
        assert_eq!(p.status, DcaStatus::Paused);
        assert_eq!(p.next_due_at, 2_000 + 3_600);
        assert_eq!(p.slice_log.len() as u32, MAX_CONSECUTIVE_FAILURES);
        assert!(!p.slice_log[0].error.contains("deferred"));
    }

    #[test]
    fn slice_log_stays_bounded() {
        let mut p = plan();
        for _ in 0..MAX_SLICE_LOG + 5 {
            apply_refusal(&mut p, 1, SliceRefusal::Deferred, "x", &U256::ZERO);
        }
        assert_eq!(p.slice_log.len(), MAX_SLICE_LOG);
    }
}
