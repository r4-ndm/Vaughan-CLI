//! Sentient-profile MCP auto-exec — skip approval cards; still re-sim + policy.

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

/// Soft policy gate before sentient auto-broadcast (re-sim still runs in execute).
pub fn gate_sentient_proposal(
    wallet: &WalletState,
    handle: &Handle,
    proposal: &TxProposal,
) -> Result<(), ProviderError> {
    let dir = vaughan_agent::paths::profile_dir(wallet.path());
    let rpc_count = 1usize.saturating_add(wallet.networks().active().fallback_rpc_urls.len());
    let config = breaker_config_for_session(Some(&dir), rpc_count).map_err(|e| {
        ProviderError::InvalidParams(format!("sentient policy: {e}"))
    })?;
    let breaker = CircuitBreaker::new(config);

    if breaker.is_tripped() {
        return Err(ProviderError::InvalidParams(format!(
            "circuit_breaker_tripped: {}",
            breaker.trip_reason().unwrap_or_else(|| "halted".into())
        )));
    }

    let trade_amount = native_sized_amount(proposal);
    if trade_amount > U256::ZERO {
        let bal = handle
            .block_on(wallet.balance())
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        let balance = U256::from_str_radix(bal.raw.trim_start_matches("0x"), 10)
            .or_else(|_| U256::from_str_radix(bal.raw.trim_start_matches("0x"), 16))
            .unwrap_or(U256::ZERO);
        breaker
            .validate_trade(trade_amount, balance, 0)
            .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
    }

    if breaker.config().enforcement == EnforcementMode::Disabled {
        tracing::warn!(
            target: "vaughan_tui::mcp",
            "sentient auto-exec with enforcement=disabled"
        );
    }

    Ok(())
}

fn native_sized_amount(proposal: &TxProposal) -> U256 {
    if proposal.value_wei > U256::ZERO {
        return proposal.value_wei;
    }
    match &proposal.proposal_type {
        ProposalType::NativeTransfer { amount_wei, .. } => *amount_wei,
        _ => U256::ZERO,
    }
}

/// Re-sim, policy gate, sign, and broadcast without showing an approval card.
pub fn auto_exec_mcp_proposal(
    wallet: &WalletState,
    handle: &Handle,
    kind: &ApprovalKind,
) -> Result<String, ProviderError> {
    let ApprovalKind::McpProposal { proposal, .. } = kind else {
        return Err(ProviderError::Internal(
            "auto_exec_mcp_proposal requires McpProposal".into(),
        ));
    };
    gate_sentient_proposal(wallet, handle, proposal)?;
    provider::execute_approval_sync(kind, wallet, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentient_and_legacy_degen_auto_exec() {
        assert!(mcp_auto_exec_enabled("sentient"));
        assert!(mcp_auto_exec_enabled("degen"));
        assert!(!mcp_auto_exec_enabled("default"));
        assert!(!mcp_auto_exec_enabled("savings"));
    }
}
