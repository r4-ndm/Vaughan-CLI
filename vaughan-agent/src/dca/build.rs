//! Build / validate a DCA plan draft (shared by TUI + MCP).

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, U256};

use super::types::{DcaPlan, DcaPlanProposal, DcaStatus, DcaTrigger, DcaVenue, MIN_INTERVAL_SECS};
use crate::error::AgentError;

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validate and assemble a [`DcaPlan`] (status active, schedule starts now).
///
/// `chain_id` and `account` bind the plan so a later fire cannot spend on the
/// wrong network or a switched F3 wallet.
#[allow(clippy::too_many_arguments)]
pub fn build_plan(
    token_out: &str,
    slice_amount_wei: &str,
    interval_secs: u64,
    venue: DcaVenue,
    max_slippage_bps: u32,
    max_slices: u32,
    dry_run: bool,
    chain_id: u64,
    account: Address,
) -> Result<DcaPlan, AgentError> {
    let addr = Address::from_str(token_out.trim())
        .map_err(|e| AgentError::InvalidToolCall(format!("token_out: {e}")))?;
    if addr.is_zero() {
        return Err(AgentError::InvalidToolCall(
            "token_out must be an ERC-20 (not native)".into(),
        ));
    }
    if account.is_zero() {
        return Err(AgentError::InvalidToolCall(
            "account must be a non-zero wallet address".into(),
        ));
    }
    let amount = U256::from_str(slice_amount_wei.trim())
        .map_err(|e| AgentError::InvalidToolCall(format!("slice_amount_wei: {e}")))?;
    if amount.is_zero() {
        return Err(AgentError::InvalidToolCall(
            "slice_amount_wei must be > 0".into(),
        ));
    }
    if interval_secs < MIN_INTERVAL_SECS {
        return Err(AgentError::InvalidToolCall(format!(
            "interval_secs must be ≥ {MIN_INTERVAL_SECS} (15 minutes)"
        )));
    }
    if max_slices == 0 {
        return Err(AgentError::InvalidToolCall("max_slices must be ≥ 1".into()));
    }
    if max_slippage_bps > 10_000 {
        return Err(AgentError::InvalidToolCall(
            "max_slippage_bps must be ≤ 10000".into(),
        ));
    }
    // Resolve venue early so bad labels fail at create time.
    let _ = venue.resolve().map_err(AgentError::InvalidToolCall)?;
    venue
        .validate_for_chain(chain_id)
        .map_err(AgentError::InvalidToolCall)?;

    let now = now_unix();
    let id = format!(
        "dca-{now:x}-{:04x}",
        (amount.as_limbs()[0] as u16) ^ (interval_secs as u16)
    );
    Ok(DcaPlan {
        id,
        chain_id,
        account: format!("{account:#x}"),
        token_out: format!("{addr:#x}"),
        slice_amount_wei: amount.to_string(),
        trigger: DcaTrigger::Time { interval_secs },
        venue,
        max_slippage_bps,
        max_slices,
        slices_done: 0,
        spent_wei: "0".into(),
        consecutive_failures: 0,
        status: DcaStatus::Active,
        created_at: now,
        next_due_at: now, // first slice eligible immediately after approve
        dry_run,
        slice_log: vec![],
    })
}

/// Wrap a validated plan as a human-approval proposal.
pub fn build_proposal(plan: DcaPlan, explanation: impl Into<String>) -> DcaPlanProposal {
    let explanation = explanation.into();
    let digest = {
        let mut n: u32 = explanation.len() as u32;
        for b in plan.id.bytes() {
            n = n.wrapping_mul(31).wrapping_add(u32::from(b));
        }
        n
    };
    DcaPlanProposal {
        proposal_id: format!("dca-prop-{digest:08x}"),
        llm_explanation: explanation,
        plan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn acct() -> Address {
        address!("0x1111111111111111111111111111111111111111")
    }

    #[test]
    fn rejects_short_interval() {
        let err = build_plan(
            "0x2222222222222222222222222222222222222222",
            "1000000000000000000",
            60,
            DcaVenue::default(),
            100,
            5,
            true,
            369,
            acct(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("15 minutes"));
    }

    #[test]
    fn accepts_four_hour() {
        let p = build_plan(
            "0x2222222222222222222222222222222222222222",
            "1000000000000000000",
            14400,
            DcaVenue::default(),
            100,
            10,
            true,
            369,
            acct(),
        )
        .unwrap();
        assert_eq!(p.max_slices, 10);
        assert!(p.dry_run);
        assert_eq!(p.chain_id, 369);
    }

    #[test]
    fn rejects_agg_off_pulse() {
        let err = build_plan(
            "0x2222222222222222222222222222222222222222",
            "1000000000000000000",
            14400,
            DcaVenue::default(),
            100,
            10,
            true,
            10_001,
            acct(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("PulseChain"));
    }
}
