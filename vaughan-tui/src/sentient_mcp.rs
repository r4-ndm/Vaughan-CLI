//! Sentient-profile bridge from MCP proposals to the signer daemon.
//!
//! The TUI never signs for a sentient profile and enforces no policy itself: it maps a
//! proposal's calldata onto a typed [`SignerIntent`] ([`evm_tx_to_sentient_intent`]), and the
//! daemon (`vaughan-cli/src/signerd/`) applies every policy check and broadcasts. Anything that
//! does not map to a known intent is refused as `intent_untyped`.
//!
//! The circuit breaker is **session-scoped**: callers create one via [`new_session_breaker`]
//! and reuse it so consecutive-error tripwires accumulate across proposals.

use alloy::primitives::{Address, U256};
use alloy::sol;
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_agent::{breaker_config_for_session, CircuitBreaker, EnforcementMode};
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::is_sentient_profile;
use vaughan_core::core::{decode_v3_swap_calldata, SignerClient, SignerIntent, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::ProviderError;

use crate::provider::ApprovalKind;

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

fn format_policy_summary(
    enforcement: EnforcementMode,
    max_position_pct: u8,
    max_slippage_bps: u32,
) -> String {
    match enforcement {
        EnforcementMode::Enforced => format!(
            "policy enforced · max {}%/trade · {:.2}% slippage cap",
            max_position_pct,
            max_slippage_bps as f64 / 100.0
        ),
        EnforcementMode::WarnOnly => format!(
            "policy WARN-ONLY · max {}%/trade · {:.2}% slippage cap (breaches allowed)",
            max_position_pct,
            max_slippage_bps as f64 / 100.0
        ),
        EnforcementMode::Disabled => {
            "policy DISABLED — agent trades unbounded (Ctrl+K halts the signer daemon)".to_string()
        }
    }
}

/// One-line policy summary for a sentient session (unlock + settings).
///
/// `None` for non-sentient profiles. Warn-only / disabled enforcement is
/// called out explicitly — a silent default would hide an unbounded agent.
pub fn sentient_policy_line(wallet: &WalletState) -> Option<String> {
    if !mcp_auto_exec_enabled(wallet.profile_name()) {
        return None;
    }
    if let Ok(handle) = Handle::try_current() {
        let client = SignerClient::default();
        if client.socket_path().exists()
            && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        {
            if let Ok(resp) = tokio::task::block_in_place(|| handle.block_on(client.policy())) {
                if let Some(pol_val) = resp.policy {
                    if let Ok(policy) = serde_json::from_value::<
                        vaughan_agent::sentient::AgentSessionPolicy,
                    >(pol_val)
                    {
                        return Some(format_policy_summary(
                            policy.enforcement,
                            policy.max_position_pct,
                            policy.max_slippage_bps,
                        ));
                    }
                }
            }
        }
    }
    let dir = vaughan_agent::paths::profile_dir(wallet.path());
    let rpc_count = 1usize.saturating_add(wallet.networks().active().fallback_rpc_urls.len());
    Some(match breaker_config_for_session(Some(&dir), rpc_count) {
        Ok(cfg) => {
            format_policy_summary(cfg.enforcement, cfg.max_position_pct, cfg.max_slippage_bps)
        }
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
}
