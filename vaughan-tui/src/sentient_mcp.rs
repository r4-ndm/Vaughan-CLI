//! Sentient-profile MCP auto-exec — skip approval cards; still re-sim + policy.
//!
//! The circuit breaker is **session-scoped**: callers create one via
//! [`new_session_breaker`] and reuse it for every auto-exec so cumulative gas
//! and consecutive-error tripwires actually accumulate across proposals.

use alloy::primitives::{Address, U256};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_agent::{breaker_config_for_session, CircuitBreaker, EnforcementMode};
use vaughan_core::core::is_sentient_profile;
use vaughan_core::core::proposal::{ProposalType, TxProposal};
use vaughan_core::core::{
    apply_proposal, fee_spike_exceeds_threshold, guard_mainnet_write, is_allowed_dex_router,
    quote_v2_exact_in, OperatingMode, WalletState,
};
use vaughan_provider::ProviderError;

use crate::provider::{self, ApprovalKind};

/// True when this vault profile should auto-sign MCP proposals (no human card).
pub fn mcp_auto_exec_enabled(profile_name: &str) -> bool {
    vaughan_core::core::sentient_mode_enabled() && is_sentient_profile(profile_name)
}

/// One-line policy summary for a sentient session (unlock + settings).
///
/// `None` for non-sentient profiles. Warn-only / disabled enforcement is
/// called out explicitly — a silent default would hide an unbounded agent.
pub fn sentient_policy_line(wallet: &WalletState) -> Option<String> {
    if !mcp_auto_exec_enabled(wallet.profile_name()) {
        return None;
    }
    let dir = vaughan_agent::paths::profile_dir(wallet.path());
    let rpc_count = 1usize.saturating_add(wallet.networks().active().fallback_rpc_urls.len());
    Some(match breaker_config_for_session(Some(&dir), rpc_count) {
        Ok(cfg) => match cfg.enforcement {
            EnforcementMode::Enforced => format!(
                "policy enforced · max {}%/trade · {:.2}% slippage cap",
                cfg.max_position_pct,
                cfg.max_slippage_bps as f64 / 100.0
            ),
            EnforcementMode::WarnOnly => format!(
                "policy WARN-ONLY · max {}%/trade · {:.2}% slippage cap (breaches allowed)",
                cfg.max_position_pct,
                cfg.max_slippage_bps as f64 / 100.0
            ),
            EnforcementMode::Disabled => {
                "policy DISABLED — agent trades unbounded (Ctrl+K still stops)".to_string()
            }
        },
        Err(e) => format!("policy unreadable — auto-exec fails closed ({e})"),
    })
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

/// Policy gate before sentient auto-broadcast (re-sim still runs in execute).
///
/// Returns the **fresh** fee estimate used for gas-ceiling accounting; the
/// caller records it on the breaker after a successful broadcast. Fails
/// closed: any unreadable balance, failed quote, or failed fee estimate
/// rejects the proposal rather than skipping a check.
pub fn gate_sentient_proposal(
    wallet: &WalletState,
    handle: &Handle,
    breaker: &CircuitBreaker,
    proposal: &TxProposal,
) -> Result<U256, ProviderError> {
    // Mode-keyed, not just name-keyed: the profile name got the request here,
    // and the session mode (bound at unlock, immutable per FR-5.1) must agree.
    if wallet.operating_mode() != OperatingMode::SentientTrader {
        return Err(ProviderError::Unauthorized(
            "sentient auto-exec requires the SentientTrader session mode".into(),
        ));
    }

    // Defense in depth: MCP entry points check this, but a proposal can also
    // arrive via a direct (HMAC-valid) queue write that skipped dispatch.
    guard_mainnet_write(wallet.networks().active().is_testnet)
        .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;

    if breaker.is_tripped() {
        return Err(ProviderError::InvalidParams(format!(
            "circuit_breaker_tripped: {}",
            breaker.trip_reason().unwrap_or_else(|| "halted".into())
        )));
    }

    // Per-asset position sizing. Every leg must fit its own balance.
    let legs = sizeable_legs(wallet, handle, proposal)?;
    if legs.is_empty() {
        // Arbitrary call with no sizeable value leg: the position limit
        // cannot be applied, so blind auto-exec is refused. The agent must
        // use a typed proposal (transfer / swap / batch) instead.
        return Err(ProviderError::InvalidParams(
            "sentient auto-exec requires a sizeable value leg (native value, token \
             transfer, or typed swap); raw contract calls need a human profile"
                .into(),
        ));
    }
    for (amount, balance) in &legs {
        breaker
            .validate_trade(*amount, *balance, 0)
            .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
    }

    // DexSwap: audited-router allowlist + fresh-quote slippage floor.
    if let ProposalType::DexSwap {
        router,
        path,
        amount_in,
        min_amount_out,
    } = &proposal.proposal_type
    {
        let net = wallet.networks().active();
        if !is_allowed_dex_router(net.chain_id, *router) {
            return Err(ProviderError::InvalidParams(format!(
                "router {router} is not on the audited DEX allowlist for chain {}",
                net.chain_id
            )));
        }
        let quote = handle
            .block_on(quote_v2_exact_in(&net.rpc_url, *router, *amount_in, path))
            .map_err(|e| {
                ProviderError::Internal(format!(
                    "fresh swap quote failed (fail-closed): {}",
                    e.user_message()
                ))
            })?;
        // Policy floor: min_amount_out must be within max_slippage_bps of the
        // fresh quote — the agent cannot set min_out = 0 and get sandwiched.
        let max_bps = u64::from(breaker.config().max_slippage_bps.min(10_000));
        let floor = quote.amount_out * U256::from(10_000u64 - max_bps) / U256::from(10_000u64);
        if *min_amount_out < floor {
            return Err(ProviderError::InvalidParams(format!(
                "min_amount_out {min_amount_out} is below the policy floor {floor} \
                 (fresh quote {} less {max_bps} bps slippage) — re-propose with a \
                 tighter bound",
                quote.amount_out
            )));
        }
    }

    // Fresh fee estimate (fail-closed) → spike check → pre-broadcast gas budget.
    let fresh_wei = fresh_fee_estimate(wallet, handle, proposal)?;
    if fee_spike_exceeds_threshold(proposal.estimated_fee_wei, fresh_wei) {
        return Err(ProviderError::InvalidParams(
            "network fee is unverified or increased more than 10% since the agent \
             proposal — re-propose with a fresh estimate"
                .into(),
        ));
    }
    breaker
        .check_gas_budget(fresh_wei)
        .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;

    if breaker.config().enforcement == EnforcementMode::Disabled {
        tracing::warn!(
            target: "vaughan_tui::mcp",
            "sentient auto-exec with enforcement=disabled"
        );
    }

    Ok(fresh_wei)
}

/// Fresh fee estimate for the proposal, fail-closed on error.
///
/// Batch7702 cannot `eth_estimateGas` (dummy-sig calldata), so it uses the
/// pinned-gas Ambire self-pay estimate — the same number the broadcast path
/// uses for its spike check.
fn fresh_fee_estimate(
    wallet: &WalletState,
    handle: &Handle,
    proposal: &TxProposal,
) -> Result<U256, ProviderError> {
    if matches!(proposal.proposal_type, ProposalType::Batch7702 { .. }) {
        let txns = vaughan_aa::decode_execute(&proposal.calldata)
            .map_err(|e| ProviderError::Internal(format!("batch7702 decode: {e}")))?;
        let account = wallet
            .active_address()
            .ok()
            .and_then(|a| Address::from_str(a).ok())
            .ok_or_else(|| ProviderError::Internal("no active account".into()))?;
        let adapter = handle
            .block_on(wallet.active_adapter())
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        let scw = vaughan_aa::ScwTransaction {
            account,
            chain_id: wallet.networks().active().chain_id,
            nonce: handle
                .block_on(vaughan_aa::get_account_nonce(&adapter, account))
                .unwrap_or(0),
            txns,
        };
        // Placeholder signature — same length as real `r‖s‖v‖mode`; the gas
        // limit is pinned, so only calldata shape matters for pricing.
        let (gas_limit, max_fee, _) = handle
            .block_on(vaughan_aa::estimate_self_pay_fee(
                &adapter, &scw, &[0u8; 66], None,
            ))
            .map_err(|e| {
                ProviderError::Internal(format!("batch fee estimate failed (fail-closed): {e}"))
            })?;
        return Ok(U256::from(gas_limit).saturating_mul(U256::from(max_fee)));
    }

    let evm =
        apply_proposal(wallet, proposal).map_err(|e| ProviderError::Internal(e.user_message()))?;
    let fee = handle
        .block_on(wallet.estimate_transaction_fee(evm))
        .map_err(|e| {
            ProviderError::InvalidParams(format!(
                "fresh fee estimation failed (fail-closed): {}",
                e.user_message()
            ))
        })?;
    fee.total_wei_evm()
        .ok_or_else(|| ProviderError::Internal("fee estimate missing EVM total".into()))
}

/// Amount-at-risk legs and the balance each is measured against.
///
/// One entry per (asset, amount) leg: native value, token transfers, and swap
/// inputs. Token legs are capped as a percentage of that token's own balance —
/// a per-asset position limit that needs no price oracle. An empty result
/// means the proposal carries no sizeable leg (zero-value raw contract call).
fn sizeable_legs(
    wallet: &WalletState,
    handle: &Handle,
    proposal: &TxProposal,
) -> Result<Vec<(U256, U256)>, ProviderError> {
    let native_balance = |handle: &Handle| -> Result<U256, ProviderError> {
        let bal = handle
            .block_on(wallet.balance())
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        parse_balance_raw(&bal.raw)
    };
    let token_balance = |handle: &Handle, token: &str| -> Result<U256, ProviderError> {
        let bal = handle
            .block_on(wallet.token_balance(token))
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        parse_balance_raw(&bal.raw)
    };

    match &proposal.proposal_type {
        ProposalType::Batch7702 { .. } => batch_legs(wallet, handle, proposal),
        ProposalType::NativeTransfer { amount_wei, .. } => {
            Ok(vec![(*amount_wei, native_balance(handle)?)])
        }
        ProposalType::Erc20Transfer { token, amount, .. } => {
            Ok(vec![(*amount, token_balance(handle, &token.to_string())?)])
        }
        ProposalType::DexSwap {
            path, amount_in, ..
        } => {
            if proposal.value_wei > U256::ZERO {
                // Native-in swap: the value leg is the amount at risk.
                return Ok(vec![(proposal.value_wei, native_balance(handle)?)]);
            }
            match path.first() {
                Some(input) => Ok(vec![(
                    *amount_in,
                    token_balance(handle, &input.to_string())?,
                )]),
                None => Ok(Vec::new()),
            }
        }
        ProposalType::ContractCall { .. } => {
            if proposal.value_wei > U256::ZERO {
                Ok(vec![(proposal.value_wei, native_balance(handle)?)])
            } else {
                Ok(Vec::new())
            }
        }
        ProposalType::TokenLaunch { .. } => Ok(Vec::new()),
        ProposalType::LpDeployStep { .. } => Ok(Vec::new()),
    }
}

/// Size every leg of a decoded `execute(txns)` batch per asset.
///
/// Only plain native transfers and ERC-20 `transfer` legs are sizeable; any
/// other calldata (approvals, router calls, unknown selectors) makes the
/// batch unsizeable and auto-exec refuses it — same posture as ContractCall.
fn batch_legs(
    wallet: &WalletState,
    handle: &Handle,
    proposal: &TxProposal,
) -> Result<Vec<(U256, U256)>, ProviderError> {
    let txns = vaughan_aa::decode_execute(&proposal.calldata)
        .map_err(|e| ProviderError::InvalidParams(format!("batch7702 decode: {e}")))?;
    if txns.is_empty() {
        return Err(ProviderError::InvalidParams(
            "batch7702: decoded execute had zero calls".into(),
        ));
    }

    let mut native_total = U256::ZERO;
    let mut token_totals: Vec<(Address, U256)> = Vec::new();
    for tx in &txns {
        if tx.value > U256::ZERO {
            native_total = native_total.checked_add(tx.value).ok_or_else(|| {
                ProviderError::InvalidParams("batch7702 native value overflow".into())
            })?;
        }
        if tx.data.is_empty() {
            continue;
        }
        let Some(amount) = decode_erc20_transfer_amount(&tx.data) else {
            return Err(ProviderError::InvalidParams(
                "batch7702 contains a call that is not a plain native or ERC-20 \
                 transfer — auto-exec cannot size it; use a human profile"
                    .into(),
            ));
        };
        match token_totals.iter_mut().find(|(t, _)| *t == tx.to) {
            Some((_, total)) => {
                *total = total.checked_add(amount).ok_or_else(|| {
                    ProviderError::InvalidParams("batch7702 token amount overflow".into())
                })?;
            }
            None => token_totals.push((tx.to, amount)),
        }
    }

    let mut legs = Vec::new();
    if native_total > U256::ZERO {
        let bal = handle
            .block_on(wallet.balance())
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        legs.push((native_total, parse_balance_raw(&bal.raw)?));
    }
    for (token, total) in token_totals {
        let bal = handle
            .block_on(wallet.token_balance(&token.to_string()))
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        legs.push((total, parse_balance_raw(&bal.raw)?));
    }
    Ok(legs)
}

/// Decode the amount from ERC-20 `transfer(address,uint256)` calldata.
fn decode_erc20_transfer_amount(data: &[u8]) -> Option<U256> {
    const TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];
    if data.len() != 68 || data[..4] != TRANSFER_SELECTOR {
        return None;
    }
    Some(U256::from_be_slice(&data[36..68]))
}

/// Parse a balance string, fail-closed: an unparseable balance must reject
/// the proposal, never silently skip the position check as zero.
fn parse_balance_raw(raw: &str) -> Result<U256, ProviderError> {
    let trimmed = raw.trim();
    let parsed = match trimmed.strip_prefix("0x") {
        Some(hex) if !hex.is_empty() => U256::from_str_radix(hex, 16),
        None if !trimmed.is_empty() => U256::from_str(trimmed),
        _ => return Err(unparseable(trimmed)),
    };
    parsed.map_err(|_| unparseable(trimmed))
}

fn unparseable(raw: &str) -> ProviderError {
    ProviderError::Internal(format!("unparseable balance string (fail-closed): {raw:?}"))
}

/// Re-sim, policy gate, sign, and broadcast without showing an approval card.
///
/// Records the outcome on the session breaker: the **fresh** fee estimate
/// against the gas ceiling on success (never the agent-stamped field), and
/// the consecutive-error tripwire on failure.
pub fn auto_exec_mcp_proposal(
    wallet: &mut WalletState,
    handle: &Handle,
    breaker: &CircuitBreaker,
    kind: &ApprovalKind,
) -> Result<String, ProviderError> {
    let ApprovalKind::McpProposal { proposal, .. } = kind else {
        return Err(ProviderError::Internal(
            "auto_exec_mcp_proposal requires McpProposal".into(),
        ));
    };
    let fresh_fee_wei = gate_sentient_proposal(wallet, handle, breaker, proposal)?;
    let result = provider::execute_approval_sync(kind, wallet, handle);
    match &result {
        Ok(_) => {
            if let Err(e) = breaker.record_success(fresh_fee_wei) {
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
    wallet: &mut WalletState,
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
        assert!(!mcp_auto_exec_enabled("sentient"));
        assert!(!mcp_auto_exec_enabled("degen"));
        assert!(!mcp_auto_exec_enabled("default"));
        assert!(!mcp_auto_exec_enabled("savings"));
    }

    #[test]
    fn auto_exec_when_sentient_mode_re_enabled() {
        if !vaughan_core::core::sentient_mode_enabled() {
            return;
        }
        assert!(mcp_auto_exec_enabled("sentient"));
        assert!(mcp_auto_exec_enabled("degen"));
    }

    #[test]
    fn erc20_transfer_amount_decodes() {
        // transfer(address,uint256): selector + 32-byte address + 32-byte amount.
        let mut data = vec![0xa9, 0x05, 0x9c, 0xbb];
        data.extend_from_slice(&[0u8; 32]);
        data.extend_from_slice(&U256::from(42_000u64).to_be_bytes::<32>());
        assert_eq!(
            decode_erc20_transfer_amount(&data),
            Some(U256::from(42_000u64))
        );
    }

    #[test]
    fn erc20_transfer_amount_rejects_other_selectors_and_lengths() {
        // approve(address,uint256) selector.
        let mut approve = vec![0x09, 0x5e, 0xa7, 0xb3];
        approve.extend_from_slice(&[0u8; 64]);
        assert_eq!(decode_erc20_transfer_amount(&approve), None);
        // Right selector, truncated payload.
        assert_eq!(
            decode_erc20_transfer_amount(&[0xa9, 0x05, 0x9c, 0xbb]),
            None
        );
        // Empty calldata.
        assert_eq!(decode_erc20_transfer_amount(&[]), None);
    }

    #[test]
    fn balance_parse_is_prefix_aware_and_fail_closed() {
        assert_eq!(parse_balance_raw("1000").unwrap(), U256::from(1000u64));
        // 0x1000 is hex 4096, never decimal 1000.
        assert_eq!(parse_balance_raw("0x1000").unwrap(), U256::from(4096u64));
        assert!(parse_balance_raw("not-a-number").is_err());
        assert!(parse_balance_raw("").is_err());
    }
}
