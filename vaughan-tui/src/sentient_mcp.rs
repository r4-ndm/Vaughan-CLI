//! Sentient-profile MCP auto-exec — skip approval cards; still re-sim + policy.
//!
//! The circuit breaker is **session-scoped**: callers create one via
//! [`new_session_breaker`] and reuse it for every auto-exec so cumulative gas
//! and consecutive-error tripwires actually accumulate across proposals.

use alloy::primitives::U256;
use tokio::runtime::Handle;
use vaughan_agent::{breaker_config_for_session, CircuitBreaker, EnforcementMode};
use vaughan_core::core::is_sentient_profile;
use vaughan_core::core::proposal::{ProposalType, TxProposal};
use vaughan_core::core::WalletState;
use vaughan_provider::ProviderError;

use crate::provider::{self, ApprovalKind};

/// True when this vault profile should auto-sign MCP proposals (no human card).
pub fn mcp_auto_exec_enabled(profile_name: &str) -> bool {
    is_sentient_profile(profile_name)
}

/// Create the session-scoped circuit breaker for a sentient profile.
///
/// One per unlock session; dropping it on lock resets cumulative gas and the
/// consecutive-error counter, which is the intended session boundary.
pub fn new_session_breaker(wallet: &WalletState) -> Result<CircuitBreaker, ProviderError> {
    let dir = vaughan_agent::paths::profile_dir(wallet.path());
    let rpc_count = 1usize.saturating_add(wallet.networks().active().fallback_rpc_urls.len());
    let config = breaker_config_for_session(Some(&dir), rpc_count)
        .map_err(|e| ProviderError::InvalidParams(format!("sentient policy: {e}")))?;
    Ok(CircuitBreaker::new(config))
}

/// Soft policy gate before sentient auto-broadcast (re-sim still runs in execute).
pub fn gate_sentient_proposal(
    wallet: &WalletState,
    handle: &Handle,
    breaker: &CircuitBreaker,
    proposal: &TxProposal,
) -> Result<(), ProviderError> {
    if breaker.is_tripped() {
        return Err(ProviderError::InvalidParams(format!(
            "circuit_breaker_tripped: {}",
            breaker.trip_reason().unwrap_or_else(|| "halted".into())
        )));
    }

    match sized_position(wallet, handle, proposal)? {
        Some((amount, balance)) => breaker
            .validate_trade(amount, balance, 0)
            .map_err(|e| ProviderError::InvalidParams(e.to_string()))?,
        None => {
            // Arbitrary call with no sizeable value leg: the position limit
            // cannot be applied, so blind auto-exec is refused. The agent must
            // use a typed proposal (transfer / swap / batch) instead.
            return Err(ProviderError::InvalidParams(
                "sentient auto-exec requires a sizeable value leg (native value, token \
                 transfer, or typed swap); raw contract calls need a human profile"
                    .into(),
            ));
        }
    }

    if breaker.config().enforcement == EnforcementMode::Disabled {
        tracing::warn!(
            target: "vaughan_tui::mcp",
            "sentient auto-exec with enforcement=disabled"
        );
    }

    Ok(())
}

/// Amount at risk and the balance it is measured against, per proposal type.
///
/// Returns `Ok(None)` when the proposal carries no sizeable leg (a zero-value
/// raw contract call). Token legs are capped as a percentage of that token's
/// own balance — a per-asset position limit that needs no price oracle.
fn sized_position(
    wallet: &WalletState,
    handle: &Handle,
    proposal: &TxProposal,
) -> Result<Option<(U256, U256)>, ProviderError> {
    let native_balance = |handle: &Handle| -> Result<U256, ProviderError> {
        let bal = handle
            .block_on(wallet.balance())
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        Ok(parse_balance_raw(&bal.raw))
    };
    let token_balance = |handle: &Handle, token: &str| -> Result<U256, ProviderError> {
        let bal = handle
            .block_on(wallet.token_balance(token))
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        Ok(parse_balance_raw(&bal.raw))
    };

    if proposal.value_wei > U256::ZERO {
        return Ok(Some((proposal.value_wei, native_balance(handle)?)));
    }

    match &proposal.proposal_type {
        ProposalType::NativeTransfer { amount_wei, .. } => {
            Ok(Some((*amount_wei, native_balance(handle)?)))
        }
        ProposalType::Erc20Transfer { token, amount, .. } => {
            Ok(Some((*amount, token_balance(handle, &token.to_string())?)))
        }
        ProposalType::DexSwap {
            path, amount_in, ..
        } => match path.first() {
            // value_wei == 0 here, so the input leg is the first path token.
            Some(input) => Ok(Some((
                *amount_in,
                token_balance(handle, &input.to_string())?,
            ))),
            None => Ok(None),
        },
        ProposalType::Batch7702 { total_value, .. } => {
            Ok(Some((*total_value, native_balance(handle)?)))
        }
        ProposalType::ContractCall { .. } => Ok(None),
    }
}

fn parse_balance_raw(raw: &str) -> U256 {
    U256::from_str_radix(raw.trim_start_matches("0x"), 10)
        .or_else(|_| U256::from_str_radix(raw.trim_start_matches("0x"), 16))
        .unwrap_or(U256::ZERO)
}

/// Re-sim, policy gate, sign, and broadcast without showing an approval card.
///
/// Records the outcome on the session breaker: estimated fee against the gas
/// ceiling on success, and the consecutive-error tripwire on failure.
pub fn auto_exec_mcp_proposal(
    wallet: &WalletState,
    handle: &Handle,
    breaker: &CircuitBreaker,
    kind: &ApprovalKind,
) -> Result<String, ProviderError> {
    let ApprovalKind::McpProposal { proposal, .. } = kind else {
        return Err(ProviderError::Internal(
            "auto_exec_mcp_proposal requires McpProposal".into(),
        ));
    };
    gate_sentient_proposal(wallet, handle, breaker, proposal)?;
    let result = provider::execute_approval_sync(kind, wallet, handle);
    match &result {
        Ok(_) => {
            let gas = proposal.estimated_fee_wei.unwrap_or(U256::ZERO);
            if let Err(e) = breaker.record_success(gas) {
                tracing::warn!(target: "vaughan_tui::mcp", "breaker tripped after broadcast: {e}");
            }
        }
        Err(e) => breaker.record_failure(&e.to_string()),
    }
    result
}

/// Breaker check for sentient stealth sweeps (no TxProposal to size).
///
/// Sweeps move the full note balance by construction, so position sizing does
/// not apply; the trip state and failure tripwire still do.
pub fn auto_exec_stealth_sweep(
    wallet: &WalletState,
    handle: &Handle,
    breaker: &CircuitBreaker,
    kind: &ApprovalKind,
) -> Result<String, ProviderError> {
    if breaker.is_tripped() {
        return Err(ProviderError::InvalidParams(format!(
            "circuit_breaker_tripped: {}",
            breaker.trip_reason().unwrap_or_else(|| "halted".into())
        )));
    }
    let result = provider::execute_approval_sync(kind, wallet, handle);
    match &result {
        Ok(_) => {
            let _ = breaker.record_success(U256::ZERO);
        }
        Err(e) => breaker.record_failure(&e.to_string()),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_degen_profile_still_enables_auto_exec() {
        assert!(mcp_auto_exec_enabled("sentient"));
        assert!(mcp_auto_exec_enabled("degen"));
        assert!(!mcp_auto_exec_enabled("default"));
        assert!(!mcp_auto_exec_enabled("savings"));
    }
}
