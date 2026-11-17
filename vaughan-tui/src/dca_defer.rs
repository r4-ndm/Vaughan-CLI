//! Classify a signer-daemon refusal of one DCA slice: DEFERRED or FAILED.
//!
//! Single purpose: turn the `{code}: {message}` string the daemon sends back
//! (`vaughan_core::core::signer_client`) into the [`SliceRefusal`] that
//! `vaughan_agent::dca::defer` then writes into the plan. A gas refusal moved no
//! money, so counting it would pause a healthy plan after three refusals and burn
//! three buy windows.
//!
//! Must NOT contain: plan mutation (that is `vaughan_agent::dca::defer`), IPC, or
//! DCA scheduling.

use vaughan_agent::dca::SliceRefusal;
use vaughan_core::core::signer_client::{error_code, retry_class, RetryClass};

/// Classify a refusal string from `SignerClient::execute`.
///
/// Deferred: `gas_ceiling_exceeded` (shrink/wait) and `gas_unavailable` (back off) — in
/// both the daemon refused before signing and the next window may well work. Paused:
/// [`RetryClass::Stop`] — `circuit_breaker_tripped` (the trip is durable and there is no
/// untrip op) and `policy_load_failed` (no usable policy on disk). Deferring either would
/// re-ask a halted daemon every 60 s forever and leave the plan reading `Active`; nothing
/// moved, so the plan is paused, not failed — an operator clears the trip or restores the
/// policy file.
/// Everything else counts, including `broadcast_uncertain`
/// ([`RetryClass::StopOperatorLooks`]): that one may have executed, so the slice must
/// stay a failure and burn its window rather than be retried.
pub fn classify(refusal: &str) -> SliceRefusal {
    match error_code(refusal).map(retry_class) {
        Some(RetryClass::ShrinkOrWait | RetryClass::BackOff) => SliceRefusal::Deferred,
        Some(RetryClass::Stop) => SliceRefusal::Paused,
        _ => SliceRefusal::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gas_and_trip_refusals_are_deferred() {
        assert_eq!(
            classify("gas_ceiling_exceeded: max_session_gas_wei ceiling 5 wei would be exceeded"),
            SliceRefusal::Deferred
        );
        assert_eq!(
            classify("gas_unavailable: fee could not be priced"),
            SliceRefusal::Deferred
        );
        assert_eq!(
            classify("circuit_breaker_tripped: Trading halted: kill switch: Ctrl+K"),
            SliceRefusal::Paused
        );
    }

    /// A store with no usable policy signed nothing, so the slice must not burn a window
    /// or count toward the three-failure pause — it halts until an operator restores it.
    #[test]
    fn a_refusal_before_policy_was_in_force_pauses_rather_than_counting() {
        assert_eq!(
            classify("policy_load_failed: no sentient-policy.toml in /tmp/store"),
            SliceRefusal::Paused
        );
    }

    #[test]
    fn a_refusal_that_may_have_moved_money_still_counts() {
        assert_eq!(
            classify("broadcast_uncertain: timed out after send"),
            SliceRefusal::Failed
        );
    }

    #[test]
    fn unknown_and_unprefixed_errors_still_count() {
        assert_eq!(
            classify("broadcast_failed: nonce too low"),
            SliceRefusal::Failed
        );
        assert_eq!(
            classify("spend_already_reserved: dca_x_0"),
            SliceRefusal::Failed
        );
        assert_eq!(
            classify("signer_unavailable: no socket"),
            SliceRefusal::Failed
        );
        assert_eq!(classify("no code here"), SliceRefusal::Failed);
        assert_eq!(classify(""), SliceRefusal::Failed);
    }
}
