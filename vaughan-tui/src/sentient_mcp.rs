//! Sentient-profile MCP auto-exec — skip approval cards; still re-sim + policy.
//!
//! The circuit breaker is **session-scoped**: callers create one via
//! [`new_session_breaker`] and reuse it for every auto-exec so cumulative gas
//! and consecutive-error tripwires actually accumulate across proposals.

use alloy::primitives::{Address, U256};
use alloy::sol;
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_agent::{breaker_config_for_session, CircuitBreaker, EnforcementMode};
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::is_sentient_profile;
use vaughan_core::core::proposal::{DexSwapKind, ProposalType, TxProposal};
use vaughan_core::core::{
    apply_proposal, decode_v3_swap_calldata, fee_spike_exceeds_threshold, guard_mainnet_write,
    is_allowed_agg_router_on_chain, is_allowed_dex_v2_router, is_allowed_dex_v3_router,
    is_v2_swap_selector, quote_v2_exact_in, quote_v3_pool_exact_in, OperatingMode, SignerClient,
    SignerIntent, WalletState,
};
use vaughan_core::error::WalletError;
use vaughan_provider::ProviderError;

use crate::provider::{self, ApprovalKind};

sol! {
    interface IERC20Approve {
        function approve(address spender, uint256 amount) external returns (bool);
    }

    interface IWETH9 {
        function deposit() external payable;
        function withdraw(uint256 wad) external;
    }

    interface IUniswapV2RouterSwap {
        function swapExactTokensForTokens(
            uint256 amountIn,
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);
    }
}

/// Convert an EVM transaction into a typed [`SignerIntent`] for sentient profiles.
///
/// Untyped or arbitrary calldata returns an error directing the user to human/adviser approval.
pub fn evm_tx_to_sentient_intent(tx: &EvmTransaction) -> Result<SignerIntent, WalletError> {
    use alloy::sol_types::SolCall;
    use vaughan_core::core::SwapFamily;

    let to_addr = Address::from_str(&tx.to).map_err(|e| {
        WalletError::InvalidTransaction(format!("invalid to address {}: {e}", tx.to))
    })?;
    let value = U256::from_str(&tx.value)
        .map_err(|_| WalletError::InvalidAmount(format!("invalid value: {}", tx.value)))?;

    let data_bytes = match tx.data.as_deref() {
        Some(s) if !s.is_empty() => {
            let hex_str = s.strip_prefix("0x").unwrap_or(s);
            if hex_str.is_empty() {
                Vec::new()
            } else {
                hex::decode(hex_str).map_err(|e| {
                    WalletError::InvalidTransaction(format!("invalid calldata hex: {e}"))
                })?
            }
        }
        _ => Vec::new(),
    };

    if data_bytes.is_empty() {
        return Ok(SignerIntent::Transfer {
            chain_id: tx.chain_id,
            to: to_addr,
            token: None,
            amount: value,
        });
    }

    if let Ok(call) = IWETH9::withdrawCall::abi_decode(&data_bytes) {
        return Ok(SignerIntent::Unwrap {
            chain_id: tx.chain_id,
            amount: call.wad,
        });
    }

    if IWETH9::depositCall::abi_decode(&data_bytes).is_ok() {
        return Ok(SignerIntent::Wrap {
            chain_id: tx.chain_id,
            amount: value,
        });
    }

    if let Ok(call) = IERC20Approve::approveCall::abi_decode(&data_bytes) {
        return Ok(SignerIntent::Approve {
            chain_id: tx.chain_id,
            token: to_addr,
            spender: call.spender,
            amount: call.amount,
            expiry: None,
        });
    }

    if let Ok(call) = IUniswapV2RouterSwap::swapExactTokensForTokensCall::abi_decode(&data_bytes) {
        return Ok(SignerIntent::Swap {
            chain_id: tx.chain_id,
            family: SwapFamily::V2,
            router: to_addr,
            path: call.path,
            amount_in: call.amountIn,
            min_amount_out: call.amountOutMin,
            fee: None,
        });
    }

    if let Ok(p) = decode_v3_swap_calldata(&data_bytes) {
        return Ok(SignerIntent::Swap {
            chain_id: tx.chain_id,
            family: SwapFamily::V3,
            router: to_addr,
            path: p.path,
            amount_in: p.amount_in,
            min_amount_out: p.amount_out_min,
            fee: None,
        });
    }

    Err(WalletError::Other(
        "intent_untyped: arbitrary calldata requires human approval via an adviser profile (not available on sentient)"
            .into(),
    ))
}

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

    // Refuse LpDeployStep unbounded calldata
    if matches!(&proposal.proposal_type, ProposalType::LpDeployStep { .. }) {
        return Err(ProviderError::InvalidParams(
            "lp_step_unbounded: LP deploy step carries deposit calldata; auto-exec refuses until sized"
                .into(),
        ));
    }

    let profile_dir = vaughan_agent::paths::profile_dir(wallet.path());
    let policy = vaughan_agent::sentient::load_policy(&profile_dir)
        .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;

    // Destination allowlist check
    if proposal.stealth_note.is_some() {
        let ProposalType::NativeTransfer { to, .. } = &proposal.proposal_type else {
            return Err(ProviderError::InvalidParams(
                "destination_not_allowed: stealth transfer to an address that was not minted by propose_stealth_send".into(),
            ));
        };
        if *to != proposal.to {
            return Err(ProviderError::InvalidParams(format!(
                "envelope mismatch: proposal.to {:#x} != variant to {:#x}",
                proposal.to, to
            )));
        }
        let allowed = vaughan_agent::sentient::consume_stealth_pending(&profile_dir, proposal.to)
            .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
        if !allowed {
            return Err(ProviderError::InvalidParams(
                "destination_not_allowed: stealth transfer to an address that was not minted by propose_stealth_send".into(),
            ));
        }
    } else {
        match &proposal.proposal_type {
            ProposalType::NativeTransfer { to, .. } => {
                if *to != proposal.to {
                    return Err(ProviderError::InvalidParams(format!(
                        "envelope mismatch: proposal.to {:#x} != variant to {:#x}",
                        proposal.to, to
                    )));
                }
                vaughan_agent::sentient::check_destination_allowed(&policy, *to)
                    .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
                vaughan_agent::sentient::check_destination_allowed(&policy, proposal.to)
                    .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
            }
            ProposalType::Erc20Transfer {
                token, recipient, ..
            } => {
                if proposal.to != *token {
                    return Err(ProviderError::InvalidParams(format!(
                        "envelope mismatch: proposal.to {:#x} != token contract {:#x}",
                        proposal.to, token
                    )));
                }
                let calldata_to =
                    decode_erc20_transfer_recipient(&proposal.calldata).ok_or_else(|| {
                        ProviderError::InvalidParams(
                            "invalid ERC-20 transfer calldata: cannot decode recipient".into(),
                        )
                    })?;
                if calldata_to != *recipient {
                    return Err(ProviderError::InvalidParams(format!(
                        "calldata mismatch: transfer recipient {calldata_to:#x} != variant recipient {recipient:#x}"
                    )));
                }
                vaughan_agent::sentient::check_destination_allowed(&policy, *recipient)
                    .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
            }
            ProposalType::ContractCall { target, .. } => {
                if proposal.value_wei > U256::ZERO {
                    if *target != proposal.to {
                        return Err(ProviderError::InvalidParams(format!(
                            "envelope mismatch: proposal.to {:#x} != target {:#x}",
                            proposal.to, target
                        )));
                    }
                    vaughan_agent::sentient::check_destination_allowed(&policy, *target)
                        .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
                    vaughan_agent::sentient::check_destination_allowed(&policy, proposal.to)
                        .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
                }
            }
            ProposalType::Batch7702 { .. } => {
                vaughan_agent::sentient::check_destination_allowed(&policy, proposal.to)
                    .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
                let txns = vaughan_aa::decode_execute(&proposal.calldata)
                    .map_err(|e| ProviderError::InvalidParams(format!("batch7702 decode: {e}")))?;
                for tx in txns {
                    if tx.value > U256::ZERO {
                        vaughan_agent::sentient::check_destination_allowed(&policy, tx.to)
                            .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
                    }
                    if !tx.data.is_empty() {
                        if let Some(to) = decode_erc20_transfer_recipient(&tx.data) {
                            vaughan_agent::sentient::check_destination_allowed(&policy, to)
                                .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Per-asset position sizing. Every leg must fit its own balance.
    let legs = sizeable_legs(wallet, handle, proposal)?;
    if legs.is_empty() {
        // Typed testnet workflows: TokenLaunch is allowed only when value_wei == 0
        match &proposal.proposal_type {
            ProposalType::TokenLaunch { .. } if proposal.value_wei.is_zero() => {}
            _ => {
                return Err(ProviderError::InvalidParams(
                    "sentient auto-exec requires a sizeable value leg (native value, token \
                     transfer/approve/unwrap, or typed swap); raw contract calls need a human profile"
                        .into(),
                ));
            }
        }
    }
    for (_asset, amount, balance) in &legs {
        breaker
            .validate_trade(*amount, *balance, 0)
            .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
    }

    // DexSwap: audited router allowlist + slippage floor per swap family.
    // V2: fresh on-chain quote via quote_v2_exact_in capped by max_slippage_bps.
    // V3: propose-time V3 quote capped by max_slippage_bps (no V2 quoter call).
    // Aggregator: audited agg allowlist + min_amount_out > 0.
    // Unknown: refuse (fail-closed, cannot distinguish family safely).
    if let ProposalType::DexSwap {
        router,
        path,
        amount_in,
        min_amount_out,
        dex_kind,
    } = &proposal.proposal_type
    {
        let net = wallet.networks().active();
        let (rpc_url, _) = wallet.rpc_endpoints_for(net);
        match dex_kind {
            DexSwapKind::V3 { pool, fee, quote } => {
                if !is_allowed_dex_v3_router(net.chain_id, *router) {
                    return Err(ProviderError::InvalidParams(format!(
                        "router {router} is not on the audited V3 DEX allowlist for chain {}",
                        net.chain_id
                    )));
                }

                // 1. Decode V3 calldata and require encoded parameters to match proposal & wallet
                let decoded = decode_v3_swap_calldata(&proposal.calldata)
                    .map_err(|e| ProviderError::InvalidParams(e.user_message()))?;
                if decoded.amount_out_min != *min_amount_out {
                    return Err(ProviderError::InvalidParams(format!(
                        "calldata mismatch: encoded amountOutMinimum ({}) != proposal min_amount_out ({})",
                        decoded.amount_out_min, min_amount_out
                    )));
                }
                let active_addr = wallet
                    .active_address()
                    .ok()
                    .and_then(|a| Address::from_str(a).ok())
                    .ok_or_else(|| ProviderError::Internal("no active wallet".into()))?;
                if decoded.recipient != active_addr {
                    return Err(ProviderError::InvalidParams(format!(
                        "calldata mismatch: encoded recipient ({:#x}) != wallet active address ({active_addr:#x})",
                        decoded.recipient
                    )));
                }
                if decoded.path != *path {
                    return Err(ProviderError::InvalidParams(format!(
                        "calldata mismatch: encoded token path ({:?}) != proposal path ({:?})",
                        decoded.path, path
                    )));
                }

                if path.len() < 2 {
                    return Err(ProviderError::InvalidParams(
                        "V3 swap path must have at least 2 tokens".into(),
                    ));
                }

                // 2. Refuse zero quotes or zero min_amount_out
                if quote.is_zero() || min_amount_out.is_zero() {
                    return Err(ProviderError::InvalidParams(
                        "quote or min_amount_out is zero: auto-exec refuses unbounded swap with no minimum output protection"
                            .into(),
                    ));
                }

                // 3. Re-derive expected output from live pool state (slot0/liquidity)
                let (actual_pool, expected_out) = handle
                    .block_on(quote_v3_pool_exact_in(
                        &rpc_url,
                        net.chain_id,
                        path[0],
                        path[1],
                        *fee,
                        *amount_in,
                    ))
                    .map_err(|e| {
                        ProviderError::Internal(format!(
                            "fresh V3 pool quote failed (fail-closed): {}",
                            e.user_message()
                        ))
                    })?;

                if *pool != Address::ZERO && *pool != actual_pool {
                    return Err(ProviderError::InvalidParams(format!(
                        "proposal pool {pool:#x} does not match factory pool {actual_pool:#x}"
                    )));
                }

                if expected_out.is_zero() {
                    return Err(ProviderError::InvalidParams(
                        "re-derived pool quote is zero: auto-exec refuses unbounded swap with no minimum output protection"
                            .into(),
                    ));
                }

                // 4. Apply max_slippage_bps to THAT re-derived expected output
                let max_bps = u64::from(breaker.config().max_slippage_bps.min(10_000));
                let floor = expected_out * U256::from(10_000u64 - max_bps) / U256::from(10_000u64);
                if *min_amount_out < floor {
                    return Err(ProviderError::InvalidParams(format!(
                        "min_amount_out {min_amount_out} is below the policy floor {floor} \
                         (fresh pool quote {expected_out} less {max_bps} bps slippage) — re-propose with a \
                         tighter bound"
                    )));
                }
            }
            DexSwapKind::V2 => {
                let dex_ok = is_allowed_dex_v2_router(net.chain_id, *router);
                if !dex_ok {
                    return Err(ProviderError::InvalidParams(format!(
                        "router {router} is not on the audited V2 DEX allowlist for chain {}",
                        net.chain_id
                    )));
                }
                if proposal.calldata.len() < 4 || !is_v2_swap_selector(&proposal.calldata) {
                    let sel = if proposal.calldata.len() >= 4 {
                        format!("0x{}", hex::encode(&proposal.calldata[..4]))
                    } else {
                        format!("0x{}", hex::encode(&proposal.calldata))
                    };
                    return Err(ProviderError::InvalidParams(format!(
                        "calldata selector {sel} does not belong to the audited V2 swap family"
                    )));
                }
                let quote = handle
                    .block_on(quote_v2_exact_in(&rpc_url, *router, *amount_in, path))
                    .map_err(|e| {
                        ProviderError::Internal(format!(
                            "fresh swap quote failed (fail-closed): {}",
                            e.user_message()
                        ))
                    })?;
                if quote.amount_out.is_zero() || min_amount_out.is_zero() {
                    return Err(ProviderError::InvalidParams(
                        "quote or min_amount_out is zero: auto-exec refuses unbounded swap with no minimum output protection".into(),
                    ));
                }
                let max_bps = u64::from(breaker.config().max_slippage_bps.min(10_000));
                let floor =
                    quote.amount_out * U256::from(10_000u64 - max_bps) / U256::from(10_000u64);
                if *min_amount_out < floor {
                    return Err(ProviderError::InvalidParams(format!(
                        "min_amount_out {min_amount_out} is below the policy floor {floor} \
                         (fresh quote {} less {max_bps} bps slippage) — re-propose with a \
                         tighter bound",
                        quote.amount_out
                    )));
                }
            }
            DexSwapKind::Aggregator => {
                let agg_ok = is_allowed_agg_router_on_chain(net.chain_id, *router);
                if !agg_ok {
                    return Err(ProviderError::InvalidParams(format!(
                        "router {router} is not on the audited Agg allowlist for chain {}",
                        net.chain_id
                    )));
                }
                if min_amount_out.is_zero() {
                    return Err(ProviderError::InvalidParams(
                        "agg swap auto-exec requires min_amount_out > 0 (propose-time bound; \
                         fresh V2 quote does not apply to aggregator routers)"
                            .into(),
                    ));
                }
            }
            DexSwapKind::Unknown => {
                return Err(ProviderError::InvalidParams(
                    "unknown swap kind: swap proposal cannot be distinguished safely and is refused (fail-closed)"
                        .into(),
                ));
            }
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

    // Cumulative 24h spend ledger reservation across sized legs.
    // Kept strictly after all family/allowlist/floor/gas checks so refusals
    // never burn budget or proposal_id, and strictly before returning Ok for broadcast.
    vaughan_agent::sentient::reserve_legs(&profile_dir, &proposal.proposal_id, &legs, &policy)
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
            .ok_or_else(|| ProviderError::Internal("no active wallet".into()))?;
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
/// One entry per (asset_key, amount, balance) leg: native value, token transfers, sized
/// approve/unwrap, and swap inputs. Token legs are capped as a percentage of
/// that token's own balance — a per-asset position limit that needs no price
/// oracle. An empty result means the proposal carries no sizeable leg
/// (unknown zero-value raw contract call).
fn sizeable_legs(
    wallet: &WalletState,
    handle: &Handle,
    proposal: &TxProposal,
) -> Result<Vec<(String, U256, U256)>, ProviderError> {
    let chain_id = wallet.networks().active().chain_id;
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
        ProposalType::NativeTransfer { amount_wei, .. } => Ok(vec![(
            format!("{chain_id}:native"),
            *amount_wei,
            native_balance(handle)?,
        )]),
        ProposalType::Erc20Transfer { token, amount, .. } => Ok(vec![(
            format!("{chain_id}:{token:#x}"),
            *amount,
            token_balance(handle, &token.to_string())?,
        )]),
        ProposalType::DexSwap {
            path, amount_in, ..
        } => {
            if proposal.value_wei > U256::ZERO {
                // Native-in swap: the value leg is the amount at risk.
                return Ok(vec![(
                    format!("{chain_id}:native"),
                    proposal.value_wei,
                    native_balance(handle)?,
                )]);
            }
            match path.first() {
                Some(input) => Ok(vec![(
                    format!("{chain_id}:{input:#x}"),
                    *amount_in,
                    token_balance(handle, &input.to_string())?,
                )]),
                None => Ok(Vec::new()),
            }
        }
        ProposalType::ContractCall { .. } => {
            if proposal.value_wei > U256::ZERO {
                // Native wrap / payable call: value is the amount at risk.
                return Ok(vec![(
                    format!("{chain_id}:native"),
                    proposal.value_wei,
                    native_balance(handle)?,
                )]);
            }
            // Typed ERC-20 approve / WETH9 withdraw — decode amount from calldata.
            if let Some(amount) = decode_erc20_approve_amount(&proposal.calldata) {
                refuse_unlimited_approve(amount)?;
                return Ok(vec![(
                    format!("{chain_id}:{:#x}", proposal.to),
                    amount,
                    token_balance(handle, &proposal.to.to_string())?,
                )]);
            }
            if let Some(amount) = decode_weth_withdraw_amount(&proposal.calldata) {
                return Ok(vec![(
                    format!("{chain_id}:{:#x}", proposal.to),
                    amount,
                    token_balance(handle, &proposal.to.to_string())?,
                )]);
            }
            Ok(Vec::new())
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
) -> Result<Vec<(String, U256, U256)>, ProviderError> {
    let chain_id = wallet.networks().active().chain_id;
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
        legs.push((
            format!("{chain_id}:native"),
            native_total,
            parse_balance_raw(&bal.raw)?,
        ));
    }
    for (token, total) in token_totals {
        let bal = handle
            .block_on(wallet.token_balance(&token.to_string()))
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        legs.push((
            format!("{chain_id}:{token:#x}"),
            total,
            parse_balance_raw(&bal.raw)?,
        ));
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

/// Decode recipient address from ERC-20 `transfer(address,uint256)` calldata.
fn decode_erc20_transfer_recipient(data: &[u8]) -> Option<Address> {
    const TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];
    if data.len() != 68 || data[..4] != TRANSFER_SELECTOR {
        return None;
    }
    Some(Address::from_slice(&data[16..36]))
}

/// Decode amount from ERC-20 `approve(address,uint256)` (incl. revoke = 0).
fn decode_erc20_approve_amount(data: &[u8]) -> Option<U256> {
    const APPROVE_SELECTOR: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];
    if data.len() != 68 || data[..4] != APPROVE_SELECTOR {
        return None;
    }
    Some(U256::from_be_slice(&data[36..68]))
}

/// Unlimited approve is a standing drain grant — never auto-exec on Sentient.
///
/// Sized approve and revoke (`amount == 0`) stay allowed under the breaker.
/// This must run before the zero-balance position skip in [`CircuitBreaker`].
fn refuse_unlimited_approve(amount: U256) -> Result<(), ProviderError> {
    if amount == U256::MAX {
        return Err(ProviderError::InvalidParams(
            "sentient auto-exec refuses unlimited ERC-20 approve \
             (max uint256); use a sized amount or the Advisor profile"
                .into(),
        ));
    }
    Ok(())
}

/// Decode wad from WETH9 `withdraw(uint256)`.
fn decode_weth_withdraw_amount(data: &[u8]) -> Option<U256> {
    const WITHDRAW_SELECTOR: [u8; 4] = [0x2e, 0x1a, 0x7d, 0x4d];
    if data.len() != 36 || data[..4] != WITHDRAW_SELECTOR {
        return None;
    }
    Some(U256::from_be_slice(&data[4..36]))
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

/// Returns true if the error occurred after broadcast (e.g. receipt poll timeout or on-chain revert).
/// In those cases, the signed transaction was already accepted by the network, so the spend
/// reservation must NOT be released.
pub(crate) fn is_post_broadcast_error(err: &ProviderError) -> bool {
    let msg = err.to_string();
    msg.contains("timed out waiting for transaction receipt")
        || msg.contains("transaction reverted on-chain")
        || msg.contains("receipt check")
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
        Err(e) => {
            breaker.record_failure(&e.to_string());
            if !is_post_broadcast_error(e) {
                let profile_dir = vaughan_agent::paths::profile_dir(wallet.path());
                if let Err(re) =
                    vaughan_agent::sentient::release_legs(&profile_dir, &proposal.proposal_id)
                {
                    tracing::warn!(
                        target: "vaughan_tui::mcp",
                        "failed to release spend reservation for {}: {re}",
                        proposal.proposal_id
                    );
                }
            }
        }
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
    let stealth_addr = match kind {
        ApprovalKind::StealthSweep {
            stealth_address, ..
        } => stealth_address.clone(),
        _ => return Err(ProviderError::Internal("expected StealthSweep".into())),
    };
    let profile_dir = vaughan_agent::paths::profile_dir(wallet.path());
    let policy = vaughan_agent::sentient::load_policy(&profile_dir)
        .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;
    let notes = handle
        .block_on(wallet.scan_stealth_notes())
        .map_err(|e| ProviderError::Internal(e.user_message()))?;
    let note = notes
        .into_iter()
        .find(|n| {
            format!("{:#x}", n.announcement.stealth_address).eq_ignore_ascii_case(&stealth_addr)
        })
        .ok_or_else(|| {
            ProviderError::InvalidParams(format!("no unswept note for {stealth_addr}"))
        })?;

    let net = wallet.networks().active();
    let asset = format!("{}:native", net.chain_id);
    let key = format!("sweep_{stealth_addr}");
    vaughan_agent::sentient::SpendLedger::reserve(
        &profile_dir,
        &key,
        &asset,
        note.balance_wei,
        note.balance_wei,
        100, // recovery, not spend: note is its own budget
        policy.enforcement,
    )
    .map_err(|e| ProviderError::InvalidParams(e.to_string()))?;

    let stealth_addr_parsed = Address::from_str(&stealth_addr)
        .map_err(|e| ProviderError::InvalidParams(format!("bad stealth_address: {e}")))?;
    let intent = SignerIntent::SweepStealth {
        chain_id: net.chain_id,
        stealth_address: stealth_addr_parsed,
    };
    let client = SignerClient::default();
    let result = handle
        .block_on(client.execute(&intent))
        .map_err(|e| ProviderError::Unauthorized(format!("sentient_local_signing_disabled: {e}")));
    match &result {
        Ok(_) => {
            let _ = breaker.record_success(U256::ZERO);
        }
        Err(e) => {
            breaker.record_failure(&e.to_string());
            if let Err(re) = vaughan_agent::sentient::release_legs(&profile_dir, &key) {
                tracing::warn!(
                    target: "vaughan_tui::mcp",
                    "failed to release spend reservation for {key}: {re}"
                );
            }
        }
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

    #[test]
    fn test_evm_tx_to_sentient_intent_mapping() {
        use alloy::sol_types::SolCall;

        // 1. Plain transfer
        let tx = EvmTransaction {
            from: "0x1111111111111111111111111111111111111111".into(),
            to: "0x2222222222222222222222222222222222222222".into(),
            value: "1000000".into(),
            data: None,
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: 943,
        };
        let intent = evm_tx_to_sentient_intent(&tx).unwrap();
        assert!(
            matches!(intent, SignerIntent::Transfer { amount, token: None, .. } if amount == U256::from(1000000))
        );

        // 2. Wrap
        let deposit_call = IWETH9::depositCall {};
        let tx = EvmTransaction {
            from: "0x1111111111111111111111111111111111111111".into(),
            to: "0x2222222222222222222222222222222222222222".into(),
            value: "5000".into(),
            data: Some(format!("0x{}", hex::encode(deposit_call.abi_encode()))),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: 943,
        };
        let intent = evm_tx_to_sentient_intent(&tx).unwrap();
        assert!(matches!(intent, SignerIntent::Wrap { amount, .. } if amount == U256::from(5000)));

        // 3. Unwrap
        let withdraw_call = IWETH9::withdrawCall {
            wad: U256::from(3000),
        };
        let tx = EvmTransaction {
            from: "0x1111111111111111111111111111111111111111".into(),
            to: "0x2222222222222222222222222222222222222222".into(),
            value: "0".into(),
            data: Some(format!("0x{}", hex::encode(withdraw_call.abi_encode()))),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: 943,
        };
        let intent = evm_tx_to_sentient_intent(&tx).unwrap();
        assert!(
            matches!(intent, SignerIntent::Unwrap { amount, .. } if amount == U256::from(3000))
        );

        // 4. Approve
        let approve_call = IERC20Approve::approveCall {
            spender: Address::repeat_byte(0x33),
            amount: U256::from(777),
        };
        let tx = EvmTransaction {
            from: "0x1111111111111111111111111111111111111111".into(),
            to: "0x2222222222222222222222222222222222222222".into(),
            value: "0".into(),
            data: Some(format!("0x{}", hex::encode(approve_call.abi_encode()))),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: 943,
        };
        let intent = evm_tx_to_sentient_intent(&tx).unwrap();
        assert!(
            matches!(intent, SignerIntent::Approve { amount, .. } if amount == U256::from(777))
        );

        // 5. Arbitrary untyped calldata is refused
        let tx = EvmTransaction {
            from: "0x1111111111111111111111111111111111111111".into(),
            to: "0x2222222222222222222222222222222222222222".into(),
            value: "0".into(),
            data: Some("0x12345678abcdef".into()),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: 943,
        };
        let err = evm_tx_to_sentient_intent(&tx).unwrap_err();
        assert!(err
            .to_string()
            .contains("intent_untyped: arbitrary calldata requires human approval"));
    }

    #[test]
    fn auto_exec_when_sentient_mode_re_enabled() {
        assert!(vaughan_core::core::sentient_mode_enabled());
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
        // approve(address,uint256) selector — not a transfer.
        let mut approve = vec![0x09, 0x5e, 0xa7, 0xb3];
        approve.extend_from_slice(&[0u8; 64]);
        assert_eq!(decode_erc20_transfer_amount(&approve), None);
        assert_eq!(
            decode_erc20_transfer_amount(&[0xa9, 0x05, 0x9c, 0xbb]),
            None
        );
        assert_eq!(decode_erc20_transfer_amount(&[]), None);
    }

    #[test]
    fn erc20_approve_and_weth_withdraw_decode() {
        let mut approve = vec![0x09, 0x5e, 0xa7, 0xb3];
        approve.extend_from_slice(&[0u8; 32]);
        approve.extend_from_slice(&U256::from(7_000u64).to_be_bytes::<32>());
        assert_eq!(
            decode_erc20_approve_amount(&approve),
            Some(U256::from(7_000u64))
        );

        let mut withdraw = vec![0x2e, 0x1a, 0x7d, 0x4d];
        withdraw.extend_from_slice(&U256::from(99u64).to_be_bytes::<32>());
        assert_eq!(
            decode_weth_withdraw_amount(&withdraw),
            Some(U256::from(99u64))
        );
        assert_eq!(decode_weth_withdraw_amount(&[0x2e, 0x1a, 0x7d, 0x4d]), None);
        // Unknown selector → not sizeable (raw contract_call stays refused).
        assert_eq!(decode_erc20_approve_amount(&[0xde, 0xad, 0xbe, 0xef]), None);
        assert_eq!(decode_weth_withdraw_amount(&[0xde, 0xad, 0xbe, 0xef]), None);
    }

    #[test]
    fn refuse_unlimited_approve_allows_sized_and_revoke() {
        assert!(refuse_unlimited_approve(U256::ZERO).is_ok());
        assert!(refuse_unlimited_approve(U256::from(1u64)).is_ok());
        assert!(refuse_unlimited_approve(U256::MAX).is_err());
    }

    #[test]
    fn balance_parse_is_prefix_aware_and_fail_closed() {
        assert_eq!(parse_balance_raw("1000").unwrap(), U256::from(1000u64));
        // 0x1000 is hex 4096, never decimal 1000.
        assert_eq!(parse_balance_raw("0x1000").unwrap(), U256::from(4096u64));
        assert!(parse_balance_raw("not-a-number").is_err());
        assert!(parse_balance_raw("").is_err());
    }

    #[test]
    fn erc20_transfer_recipient_decodes() {
        use alloy::primitives::address;
        let target = address!("0x1111111111111111111111111111111111111111");
        let mut data = vec![0xa9, 0x05, 0x9c, 0xbb];
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(target.as_slice());
        data.extend_from_slice(&U256::from(42_000u64).to_be_bytes::<32>());
        assert_eq!(decode_erc20_transfer_recipient(&data), Some(target));
    }

    #[test]
    fn gate_sentient_refuses_lp_deploy_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut wallet = WalletState::load_with_session(
            dir.path().join("wallet.json"),
            OperatingMode::SentientTrader,
            "sentient",
        )
        .unwrap();
        wallet.set_active_network("pulsechain-testnet-v4").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let breaker = CircuitBreaker::new(vaughan_agent::CircuitBreakerConfig::default());
        let prop = TxProposal::new(
            "prop_lp",
            ProposalType::LpDeployStep {
                job_id: "job1".into(),
                step_label: "step1".into(),
            },
            Address::ZERO,
            U256::ZERO,
            alloy::primitives::Bytes::new(),
            100_000,
            true,
            "lp deploy",
        );
        let err = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &prop).unwrap_err();
        match err {
            ProviderError::InvalidParams(msg) => {
                assert!(
                    msg.starts_with("lp_step_unbounded:"),
                    "expected lp_step_unbounded:, got {msg}"
                );
            }
            other => panic!("expected InvalidParams, got {other:?}"),
        }
    }

    #[test]
    fn gate_sentient_enforces_destination_allowlist() {
        use alloy::primitives::address;
        let dir = tempfile::tempdir().unwrap();
        let mut wallet = WalletState::load_with_session(
            dir.path().join("wallet.json"),
            OperatingMode::SentientTrader,
            "sentient",
        )
        .unwrap();
        wallet.set_active_network("pulsechain-testnet-v4").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let breaker = CircuitBreaker::new(vaughan_agent::CircuitBreakerConfig::default());

        let target = address!("0x1111111111111111111111111111111111111111");
        let prop = TxProposal::new(
            "prop_transfer",
            ProposalType::NativeTransfer {
                to: target,
                amount_wei: U256::from(100),
            },
            target,
            U256::from(100),
            alloy::primitives::Bytes::new(),
            21_000,
            true,
            "transfer",
        );

        // 1. Refused when not in allow_transfer_to
        let err = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &prop).unwrap_err();
        match err {
            ProviderError::InvalidParams(msg) => {
                assert!(
                    msg.starts_with("destination_not_allowed:"),
                    "expected destination_not_allowed:, got {msg}"
                );
            }
            other => panic!("expected InvalidParams, got {other:?}"),
        }

        // 2. Value-bearing ContractCall to non-allowlisted target is refused
        let call_prop = TxProposal::new(
            "prop_call",
            ProposalType::ContractCall {
                target,
                function_name: Some("foo".into()),
            },
            target,
            U256::from(50),
            alloy::primitives::Bytes::new(),
            50_000,
            true,
            "call",
        );
        let err2 = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &call_prop).unwrap_err();
        match err2 {
            ProviderError::InvalidParams(msg) => {
                assert!(
                    msg.starts_with("destination_not_allowed:"),
                    "expected destination_not_allowed:, got {msg}"
                );
            }
            other => panic!("expected InvalidParams, got {other:?}"),
        }

        // 3. Erc20Transfer with recipient not allowlisted is refused
        let token = address!("0x2222222222222222222222222222222222222222");
        let non_allowlisted_recipient = address!("0x3333333333333333333333333333333333333333");
        let mut erc20_calldata = vec![0xa9, 0x05, 0x9c, 0xbb];
        erc20_calldata.extend_from_slice(&[0u8; 12]);
        erc20_calldata.extend_from_slice(non_allowlisted_recipient.as_slice());
        erc20_calldata.extend_from_slice(&U256::from(500).to_be_bytes::<32>());
        let erc20_prop = TxProposal::new(
            "prop_erc20",
            ProposalType::Erc20Transfer {
                token,
                recipient: non_allowlisted_recipient,
                amount: U256::from(500),
            },
            token,
            U256::ZERO,
            alloy::primitives::Bytes::from(erc20_calldata.clone()),
            65_000,
            true,
            "erc20 transfer",
        );
        let err_erc20 =
            gate_sentient_proposal(&wallet, rt.handle(), &breaker, &erc20_prop).unwrap_err();
        match err_erc20 {
            ProviderError::InvalidParams(msg) => {
                assert!(
                    msg.starts_with("destination_not_allowed:"),
                    "expected destination_not_allowed:, got {msg}"
                );
            }
            other => panic!("expected InvalidParams, got {other:?}"),
        }

        // 4. Erc20Transfer with calldata-recipient != envelope recipient is refused
        let another_recipient = address!("0x4444444444444444444444444444444444444444");
        let mismatched_erc20 = TxProposal::new(
            "prop_erc20_mismatch",
            ProposalType::Erc20Transfer {
                token,
                recipient: another_recipient,
                amount: U256::from(500),
            },
            token,
            U256::ZERO,
            alloy::primitives::Bytes::from(erc20_calldata),
            65_000,
            true,
            "erc20 mismatch",
        );
        let err_mismatch =
            gate_sentient_proposal(&wallet, rt.handle(), &breaker, &mismatched_erc20).unwrap_err();
        match err_mismatch {
            ProviderError::InvalidParams(msg) => {
                assert!(
                    msg.starts_with("calldata mismatch:"),
                    "expected calldata mismatch:, got {msg}"
                );
            }
            other => panic!("expected InvalidParams, got {other:?}"),
        }

        // 5. Batch7702 with call to non-allowlisted target is refused
        let batch_txns = vec![vaughan_aa::Transaction {
            to: non_allowlisted_recipient,
            value: U256::from(10),
            data: alloy::primitives::Bytes::new(),
        }];
        let batch_calldata = vaughan_aa::encode_execute(&batch_txns, &[0u8; 65]);
        let batch_prop = TxProposal::new(
            "prop_batch",
            ProposalType::Batch7702 {
                target_count: 1,
                total_value: U256::from(10),
            },
            target,
            U256::ZERO,
            alloy::primitives::Bytes::from(batch_calldata),
            150_000,
            true,
            "batch 7702",
        );
        let err_batch =
            gate_sentient_proposal(&wallet, rt.handle(), &breaker, &batch_prop).unwrap_err();
        match err_batch {
            ProviderError::InvalidParams(msg) => {
                assert!(
                    msg.starts_with("destination_not_allowed:"),
                    "expected destination_not_allowed:, got {msg}"
                );
            }
            other => panic!("expected InvalidParams, got {other:?}"),
        }

        // 6. Allowed when in allow_transfer_to: passes destination check and reaches RPC sizing
        let profile_dir = vaughan_agent::paths::profile_dir(wallet.path());
        let policy = vaughan_agent::sentient::AgentSessionPolicy {
            allow_transfer_to: vec![format!("{target:#x}")],
            ..Default::default()
        };
        vaughan_agent::sentient::save_policy(&profile_dir, &policy).unwrap();

        let res = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &prop);
        match res.unwrap_err() {
            ProviderError::Internal(msg) => {
                assert!(
                    !msg.contains("destination_not_allowed"),
                    "destination allowlist should have passed, but got: {msg}"
                );
            }
            other => panic!("expected ProviderError::Internal from sizing RPC, got {other:?}"),
        }
    }

    #[test]
    fn test_non_stealth_transfer_with_stealth_pay_id_prefix_still_refused() {
        use alloy::primitives::address;
        let dir = tempfile::tempdir().unwrap();
        let mut wallet = WalletState::load_with_session(
            dir.path().join("wallet.json"),
            OperatingMode::SentientTrader,
            "sentient",
        )
        .unwrap();
        wallet.set_active_network("pulsechain-testnet-v4").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let breaker = CircuitBreaker::new(vaughan_agent::CircuitBreakerConfig::default());
        let target = address!("0x1111111111111111111111111111111111111111");

        // 1. Proposal id starts with stealth_pay_, but stealth_note is None (no structural marker)
        let fake_stealth_prop = TxProposal::new(
            "stealth_pay_spoofed_id",
            ProposalType::NativeTransfer {
                to: target,
                amount_wei: U256::from(100),
            },
            target,
            U256::from(100),
            alloy::primitives::Bytes::new(),
            21_000,
            true,
            "spoofed stealth",
        );
        assert!(fake_stealth_prop.stealth_note.is_none());

        let err =
            gate_sentient_proposal(&wallet, rt.handle(), &breaker, &fake_stealth_prop).unwrap_err();
        match err {
            ProviderError::InvalidParams(msg) => {
                assert!(
                    msg.starts_with("destination_not_allowed:"),
                    "expected destination_not_allowed: for spoofed stealth proposal, got {msg}"
                );
            }
            other => panic!("expected InvalidParams, got {other:?}"),
        }

        // 2. Structural marker present (stealth_note.is_some()), but destination absent from stealth-pending.json
        let legitimate_stealth = fake_stealth_prop
            .clone()
            .with_stealth_note(alloy::primitives::B256::repeat_byte(0x55));

        let err_absent =
            gate_sentient_proposal(&wallet, rt.handle(), &breaker, &legitimate_stealth)
                .unwrap_err();
        match err_absent {
            ProviderError::InvalidParams(msg) => {
                assert_eq!(
                    msg,
                    "destination_not_allowed: stealth transfer to an address that was not minted by propose_stealth_send"
                );
            }
            other => {
                panic!("expected InvalidParams for unminted stealth destination, got {other:?}")
            }
        }

        // 3. Recorded in stealth-pending.json: passes destination check and reaches sizing
        let profile_dir = vaughan_agent::paths::profile_dir(wallet.path());
        vaughan_agent::sentient::record_stealth_pending(&profile_dir, target).unwrap();

        let res = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &legitimate_stealth);
        match res.unwrap_err() {
            ProviderError::Internal(msg) => {
                assert!(
                    !msg.contains("destination_not_allowed"),
                    "stealth destination in pending list should have passed, got: {msg}"
                );
            }
            other => panic!("expected ProviderError::Internal from sizing RPC, got {other:?}"),
        }

        // 4. Second gate attempt with the same proposal is refused because entry was consumed
        let err_consumed =
            gate_sentient_proposal(&wallet, rt.handle(), &breaker, &legitimate_stealth)
                .unwrap_err();
        match err_consumed {
            ProviderError::InvalidParams(msg) => {
                assert_eq!(
                    msg,
                    "destination_not_allowed: stealth transfer to an address that was not minted by propose_stealth_send"
                );
            }
            other => {
                panic!("expected InvalidParams after stealth destination consumed, got {other:?}")
            }
        }
    }

    #[test]
    fn test_stealth_sweep_reserves_100_pct_under_default_policy() {
        let dir = tempfile::tempdir().unwrap();
        let profile_dir = dir.path();
        let policy = vaughan_agent::sentient::AgentSessionPolicy::default();
        assert_eq!(policy.daily_cap_pct, 20);
        assert!(policy.allow_transfer_to.is_empty());

        let stealth_addr = "0x1111111111111111111111111111111111111111";
        let asset = "369:native";
        let key = format!("sweep_{stealth_addr}");
        let note_balance = U256::from(1_000_000_000_000_000_000u64); // 1 ETH

        // Under 100% cap (sweep recovery), reserving 100% of note balance succeeds under default policy
        let res = vaughan_agent::sentient::SpendLedger::reserve(
            profile_dir,
            &key,
            asset,
            note_balance,
            note_balance,
            100, // recovery cap
            policy.enforcement,
        );
        assert!(
            res.is_ok(),
            "sweep reservation must succeed under 100% recovery cap: {res:?}"
        );

        // Ledger records the amount for audit trail
        let snap = vaughan_agent::sentient::SpendLedger::snapshot(profile_dir).unwrap();
        assert_eq!(snap.assets.get(asset).unwrap().spent, note_balance);

        // Prove that under the default 20% daily cap it would have failed
        let res_failed = vaughan_agent::sentient::SpendLedger::reserve(
            profile_dir,
            "another_key",
            "369:0xother",
            note_balance,
            note_balance,
            policy.daily_cap_pct, // 20%
            policy.enforcement,
        );
        assert!(
            res_failed.is_err(),
            "20% cap should fail for 100% balance spend"
        );
        assert!(res_failed
            .unwrap_err()
            .to_string()
            .contains("spend_cap_exceeded"));
    }

    #[test]
    fn test_is_post_broadcast_error_distinguishes_post_and_pre_broadcast() {
        // Post-broadcast errors (receipt timeout, on-chain revert, receipt RPC error)
        let err_timeout =
            ProviderError::Internal("timed out waiting for transaction receipt".into());
        let err_revert_chain = ProviderError::InvalidParams(
            "transaction reverted on-chain — LP Brew step not applied; check gas and token balances, then re-propose".into(),
        );
        let err_receipt_rpc = ProviderError::Internal("receipt check: connection reset".into());
        assert!(is_post_broadcast_error(&err_timeout));
        assert!(is_post_broadcast_error(&err_revert_chain));
        assert!(is_post_broadcast_error(&err_receipt_rpc));

        // Pre-broadcast errors (simulation revert, fee spike, network rejection at broadcast time)
        let err_sim_revert = ProviderError::InvalidParams(
            "simulation reverted: execution reverted: ERC20: transfer amount exceeds balance"
                .into(),
        );
        let err_fee_spike = ProviderError::InvalidParams(
            "network fee is unverified or increased more than 10% since the agent proposal — deny and ask the agent to re-propose".into(),
        );
        let err_net_reject =
            ProviderError::Internal("network rejected the signed tx: nonce too low".into());
        let err_unauthorized = ProviderError::Unauthorized("wallet is locked".into());
        assert!(!is_post_broadcast_error(&err_sim_revert));
        assert!(!is_post_broadcast_error(&err_fee_spike));
        assert!(!is_post_broadcast_error(&err_net_reject));
        assert!(!is_post_broadcast_error(&err_unauthorized));
    }
}
