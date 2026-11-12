//! Anvil integration tests for sentient V3 swap proposal gating and auto-exec.
//!
//! Validates:
//! 1. A V3 proposal passes the gate without any V2 quoter call.
//! 2. A V3-quote slippage floor is strictly enforced.
//! 3. A V2 proposal still gets the fresh V2 quote (fails closed when unreachable/absent).
//! 4. An unknown swap kind is refused (fails closed, never falls back to V2).
//! 5. Router allowlist mismatches (V3 router for V2, or V2 router for V3) are refused.
//! 6. Negative test proving the old failure shape is gone (the reproduction from testnet 943
//!    successfully broadcasts instead of failing with "fresh swap quote failed").

mod common;

use alloy::primitives::{address, Address, Bytes, U256};
use common::lp_anvil_fixtures::{plant_factory_pool, plant_v3_pool};
use common::mock_evm::plant_code;
use common::{Anvil, PASSWORD};
use secrecy::SecretString;
use std::str::FromStr;
use vaughan_agent::sentient::{
    AgentSessionPolicy, CircuitBreaker, CircuitBreakerConfig, EnforcementMode, SpendLedger,
};
use vaughan_core::core::proposal::{DexSwapKind, ProposalType, TxProposal};
use vaughan_core::core::{OperatingMode, WalletState};
use vaughan_core::security::hd_wallet::validate_mnemonic;
use vaughan_provider::ProviderError;
use vaughan_tui::provider::ApprovalKind;
use vaughan_tui::sentient_mcp::{auto_exec_mcp_proposal, gate_sentient_proposal};

fn encode_exact_input_single_calldata(
    token_in: Address,
    token_out: Address,
    fee: u32,
    recipient: Address,
    amount_in: U256,
    amount_out_min: U256,
) -> Bytes {
    use alloy::sol_types::SolCall;
    use wiz4rd_sdk::abi::ISwapRouter;

    let params = ISwapRouter::ExactInputSingleParams {
        tokenIn: token_in,
        tokenOut: token_out,
        fee: alloy::primitives::aliases::U24::from(fee),
        recipient,
        deadline: U256::from(u64::MAX),
        amountIn: amount_in,
        amountOutMinimum: amount_out_min,
        sqrtPriceLimitX96: alloy::primitives::aliases::U160::ZERO,
    };
    Bytes::from(ISwapRouter::exactInputSingleCall { params }.abi_encode())
}

fn setup_sentient_wallet(anvil: &Anvil) -> (WalletState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.json");
    let mut wallet =
        WalletState::load_with_session(path, OperatingMode::SentientTrader, "sentient").unwrap();
    let mnemonic = validate_mnemonic(common::ANVIL_MNEMONIC).unwrap();
    wallet
        .create(&SecretString::from(PASSWORD.to_string()), mnemonic)
        .unwrap();
    wallet.set_active_network("pulsechain-testnet-v4").unwrap();
    wallet.set_rpc_override(anvil.url());

    let profile_dir = vaughan_agent::paths::profile_dir(wallet.path());
    let policy = AgentSessionPolicy {
        enforcement: EnforcementMode::Enforced,
        daily_cap_pct: 100,
        ..Default::default()
    };
    vaughan_agent::sentient::save_policy(&profile_dir, &policy).unwrap();

    (wallet, dir)
}

#[test]
fn test_v3_proposal_passes_gate_without_v2_quoter_and_enforces_quote_floor() {
    let anvil = Anvil::start();
    let (wallet, _dir) = setup_sentient_wallet(&anvil);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let breaker = CircuitBreaker::new(CircuitBreakerConfig {
        max_slippage_bps: 100, // 1%
        ..Default::default()
    });

    let router = address!("fc656c95ecd418536844feeaa46949bb9365beaf"); // Wiz4rd SwapRouter on 943
    let pool = address!("d47e01c1af55a48c11d0e324fb1853cf2501e0dc");
    let token_in = address!("70499adebb11efd915e3b69e700c331778628707"); // tWPLS
    let token_out = address!("29bab93456c0e97ee931c1554c7c215480aa7766");
    let amount_in = U256::from(500_000_000_000_000_000u64); // 0.5 PLS
    let recipient = Address::from_str(wallet.active_address().unwrap()).unwrap();

    // Plant factory returning mock pool and plant pool state on Anvil
    let sqrt = alloy::primitives::aliases::U160::from(U256::from(1u128) << 96);
    let liquidity = 10u128.pow(22);
    plant_factory_pool(&anvil, pool);
    plant_v3_pool(&anvil, pool, sqrt, liquidity, false);
    plant_code(&anvil, router, &[0x00]);

    // Re-derive expected output via pool state read
    let (p, expected_out) = rt
        .block_on(vaughan_core::core::quote_v3_pool_exact_in(
            &anvil.url(),
            943,
            token_in,
            token_out,
            500,
            amount_in,
        ))
        .unwrap();
    assert_eq!(p, pool);
    assert!(expected_out > U256::ZERO);

    // Floor at 100 bps (1%) slippage
    let floor = expected_out * U256::from(9900) / U256::from(10000);
    let below_floor = floor - U256::from(1);
    let at_floor = floor;

    // 1. Min amount out below floor is refused based on re-derived pool quote
    let loose_calldata = encode_exact_input_single_calldata(
        token_in,
        token_out,
        500,
        recipient,
        amount_in,
        below_floor,
    );
    let loose_prop = TxProposal::new(
        "prop_v3_loose",
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: below_floor,
            dex_kind: DexSwapKind::V3 {
                pool,
                fee: 500,
                quote: expected_out,
            },
        },
        router,
        amount_in, // native-in
        loose_calldata,
        350_000,
        true,
        "v3 swap loose",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let err = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &loose_prop).unwrap_err();
    match err {
        ProviderError::InvalidParams(msg) => {
            assert!(
                msg.contains("below the policy floor"),
                "expected slippage floor refusal, got: {msg}"
            );
            assert!(msg.contains("fresh pool quote"));
        }
        other => panic!("expected InvalidParams, got {other:?}"),
    }

    // 2. Calldata mismatch on amountOutMinimum is refused
    let mismatched_amount_calldata = encode_exact_input_single_calldata(
        token_in,
        token_out,
        500,
        recipient,
        amount_in,
        U256::from(1),
    );
    let mismatched_amount_prop = TxProposal::new(
        "prop_v3_mismatch_amount",
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: at_floor,
            dex_kind: DexSwapKind::V3 {
                pool,
                fee: 500,
                quote: expected_out,
            },
        },
        router,
        amount_in,
        mismatched_amount_calldata,
        350_000,
        true,
        "v3 mismatch amount",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let err_mismatch =
        gate_sentient_proposal(&wallet, rt.handle(), &breaker, &mismatched_amount_prop)
            .unwrap_err();
    assert!(err_mismatch
        .to_string()
        .contains("calldata mismatch: encoded amountOutMinimum"));

    // 3. Calldata mismatch on recipient is refused
    let wrong_recipient = Address::repeat_byte(0xee);
    let mismatched_recipient_calldata = encode_exact_input_single_calldata(
        token_in,
        token_out,
        500,
        wrong_recipient,
        amount_in,
        at_floor,
    );
    let mismatched_recipient_prop = TxProposal::new(
        "prop_v3_mismatch_recip",
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: at_floor,
            dex_kind: DexSwapKind::V3 {
                pool,
                fee: 500,
                quote: expected_out,
            },
        },
        router,
        amount_in,
        mismatched_recipient_calldata,
        350_000,
        true,
        "v3 mismatch recip",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let err_recip =
        gate_sentient_proposal(&wallet, rt.handle(), &breaker, &mismatched_recipient_prop)
            .unwrap_err();
    assert!(err_recip
        .to_string()
        .contains("calldata mismatch: encoded recipient"));

    // 4. Zero quote / zero min_out is refused
    let zero_calldata = encode_exact_input_single_calldata(
        token_in,
        token_out,
        500,
        recipient,
        amount_in,
        U256::ZERO,
    );
    let zero_prop = TxProposal::new(
        "prop_v3_zero",
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: U256::ZERO,
            dex_kind: DexSwapKind::V3 {
                pool,
                fee: 500,
                quote: U256::ZERO,
            },
        },
        router,
        amount_in,
        zero_calldata,
        350_000,
        true,
        "v3 zero",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let err_zero = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &zero_prop).unwrap_err();
    assert!(err_zero
        .to_string()
        .contains("quote or min_amount_out is zero"));

    // 5. Bound at floor passes the swap gate without calling quote_v2_exact_in.
    let tight_calldata = encode_exact_input_single_calldata(
        token_in, token_out, 500, recipient, amount_in, at_floor,
    );
    let mut tight_prop = TxProposal::new(
        "prop_v3_tight",
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: at_floor,
            dex_kind: DexSwapKind::V3 {
                pool,
                fee: 500,
                quote: expected_out,
            },
        },
        router,
        amount_in,
        tight_calldata,
        350_000,
        true,
        "v3 swap tight",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let evm = vaughan_core::core::apply_proposal(&wallet, &tight_prop).unwrap();
    let est = rt.block_on(wallet.estimate_transaction_fee(evm)).unwrap();
    tight_prop.estimated_fee_wei = est.total_wei_evm();

    let res = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &tight_prop);
    assert!(
        res.is_ok(),
        "tight V3 proposal must pass gate without V2 quoter call: {res:?}"
    );
}

#[test]
fn test_v2_proposal_still_calls_fresh_v2_quoter() {
    let anvil = Anvil::start();
    let (wallet, _dir) = setup_sentient_wallet(&anvil);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

    // PulseX V2 router on 943
    let router = address!("DaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7");
    let token_in = address!("70499adebb11efd915e3b69e700c331778628707");
    let token_out = address!("29bab93456c0e97ee931c1554c7c215480aa7766");
    let amount_in = U256::from(500_000_000_000_000_000u64);

    let v2_prop = TxProposal::new(
        "prop_v2",
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: U256::from(100),
            dex_kind: DexSwapKind::V2,
        },
        router,
        amount_in,
        Bytes::from(vec![0x7f, 0xf3, 0x6a, 0xb5, 0x00]),
        250_000,
        true,
        "v2 swap",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let err = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &v2_prop).unwrap_err();
    match err {
        ProviderError::Internal(msg) => {
            assert!(
                msg.contains("fresh swap quote failed (fail-closed)"),
                "expected fresh V2 quote failure because router has no V2 quoter on Anvil, got: {msg}"
            );
        }
        other => panic!("expected ProviderError::Internal from quoter, got {other:?}"),
    }
}

#[test]
fn test_v2_proposal_with_invalid_calldata_selector_is_refused() {
    let anvil = Anvil::start();
    let (wallet, _dir) = setup_sentient_wallet(&anvil);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

    let router = address!("DaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7");
    let token_in = address!("70499adebb11efd915e3b69e700c331778628707");
    let token_out = address!("29bab93456c0e97ee931c1554c7c215480aa7766");
    let amount_in = U256::from(500_000_000_000_000_000u64);

    let v2_prop = TxProposal::new(
        "prop_v2_bad_sel",
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: U256::from(100),
            dex_kind: DexSwapKind::V2,
        },
        router,
        amount_in,
        Bytes::from(vec![0x12, 0x34, 0x56, 0x78]),
        250_000,
        true,
        "v2 bad sel",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let err = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &v2_prop).unwrap_err();
    match err {
        ProviderError::InvalidParams(msg) => {
            assert!(
                msg.contains("does not belong to the audited V2 swap family"),
                "expected V2 selector error, got: {msg}"
            );
        }
        other => panic!("expected ProviderError::InvalidParams, got {other:?}"),
    }
}

#[test]
fn test_unknown_swap_kind_is_refused() {
    let anvil = Anvil::start();
    let (wallet, _dir) = setup_sentient_wallet(&anvil);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

    let router = address!("fc656c95ecd418536844feeaa46949bb9365beaf");
    let token_in = address!("70499adebb11efd915e3b69e700c331778628707");
    let token_out = address!("29bab93456c0e97ee931c1554c7c215480aa7766");
    let amount_in = U256::from(500_000_000_000_000_000u64);

    let unknown_prop = TxProposal::new(
        "prop_unknown",
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: U256::from(100),
            dex_kind: DexSwapKind::Unknown,
        },
        router,
        amount_in,
        Bytes::new(),
        250_000,
        true,
        "unknown swap kind",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let err = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &unknown_prop).unwrap_err();
    match err {
        ProviderError::InvalidParams(msg) => {
            assert!(
                msg.contains("unknown swap kind"),
                "expected unknown swap kind refusal, got: {msg}"
            );
        }
        other => panic!("expected ProviderError::InvalidParams, got {other:?}"),
    }
}

#[test]
fn test_router_allowlist_cross_checks() {
    let anvil = Anvil::start();
    let (wallet, _dir) = setup_sentient_wallet(&anvil);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

    let v3_router = address!("fc656c95ecd418536844feeaa46949bb9365beaf");
    let v2_router = address!("DaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7");
    let pool = address!("d47e01c1af55a48c11d0e324fb1853cf2501e0dc");
    let token_in = address!("70499adebb11efd915e3b69e700c331778628707");
    let token_out = address!("29bab93456c0e97ee931c1554c7c215480aa7766");
    let amount_in = U256::from(500_000_000_000_000_000u64);

    // 1. V3 swap proposal targeting V2 router is refused
    let v3_bad_router = TxProposal::new(
        "prop_v3_bad",
        ProposalType::DexSwap {
            router: v2_router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: U256::from(100),
            dex_kind: DexSwapKind::V3 {
                pool,
                fee: 500,
                quote: U256::from(100),
            },
        },
        v2_router,
        amount_in,
        Bytes::new(),
        350_000,
        true,
        "v3 bad router",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let err1 = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &v3_bad_router).unwrap_err();
    match err1 {
        ProviderError::InvalidParams(msg) => {
            assert!(
                msg.contains("not on the audited V3 DEX allowlist"),
                "expected V3 allowlist error, got: {msg}"
            );
        }
        other => panic!("expected InvalidParams, got {other:?}"),
    }

    // 2. V2 swap proposal targeting V3 router is refused
    let v2_bad_router = TxProposal::new(
        "prop_v2_bad",
        ProposalType::DexSwap {
            router: v3_router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: U256::from(100),
            dex_kind: DexSwapKind::V2,
        },
        v3_router,
        amount_in,
        Bytes::new(),
        250_000,
        true,
        "v2 bad router",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let err2 = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &v2_bad_router).unwrap_err();
    match err2 {
        ProviderError::InvalidParams(msg) => {
            assert!(
                msg.contains("not on the audited V2 DEX allowlist"),
                "expected V2 allowlist error, got: {msg}"
            );
        }
        other => panic!("expected InvalidParams, got {other:?}"),
    }
}

#[test]
fn test_reproduce_negative_test_proving_old_failure_shape_is_gone() {
    let anvil = Anvil::start();
    let (mut wallet, _dir) = setup_sentient_wallet(&anvil);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let breaker = CircuitBreaker::new(CircuitBreakerConfig {
        max_slippage_bps: 100, // 100 bps = 1%
        ..Default::default()
    });

    let router = address!("fc656c95ecd418536844feeaa46949bb9365beaf");
    let pool = address!("d47e01c1af55a48c11d0e324fb1853cf2501e0dc");
    let token_in = address!("70499adebb11efd915e3b69e700c331778628707"); // PLS/WPLS
    let token_out = address!("29bab93456c0e97ee931c1554c7c215480aa7766");
    let amount_in = U256::from(500_000_000_000_000_000u64); // 0.5 tPLS
    let recipient = Address::from_str(wallet.active_address().unwrap()).unwrap();

    // Plant mock pool, factory, and router code at SWAP_ROUTER_943 so gate & broadcast succeed on Anvil
    let sqrt = alloy::primitives::aliases::U160::from(U256::from(1u128) << 96);
    let liquidity = 10u128.pow(22);
    plant_factory_pool(&anvil, pool);
    plant_v3_pool(&anvil, pool, sqrt, liquidity, false);
    plant_code(&anvil, router, &[0x00]);

    let (p, expected_out) = rt
        .block_on(vaughan_core::core::quote_v3_pool_exact_in(
            &anvil.url(),
            943,
            token_in,
            token_out,
            500,
            amount_in,
        ))
        .unwrap();
    assert_eq!(p, pool);

    let min_amount_out = expected_out * U256::from(9900) / U256::from(10000); // 100 bps slippage
    let calldata = encode_exact_input_single_calldata(
        token_in,
        token_out,
        500,
        recipient,
        amount_in,
        min_amount_out,
    );

    let mut proposal = TxProposal::new(
        "v3_swap_reproduce",
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out,
            dex_kind: DexSwapKind::V3 {
                pool,
                fee: 500,
                quote: expected_out,
            },
        },
        router,
        amount_in, // native-in
        calldata,
        350_000,
        true,
        "reproduced sentient swap",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let evm = vaughan_core::core::apply_proposal(&wallet, &proposal).unwrap();
    let est = rt.block_on(wallet.estimate_transaction_fee(evm)).unwrap();
    proposal.estimated_fee_wei = est.total_wei_evm();

    let kind = ApprovalKind::McpProposal {
        proposal_id: proposal.proposal_id.clone(),
        source: "agent".into(),
        proposal: Box::new(proposal),
    };

    let result = auto_exec_mcp_proposal(&mut wallet, rt.handle(), &breaker, &kind);
    // MUST NOT fail with:
    // "internal error: fresh swap quote failed (fail-closed): Could not reach the network. Check your connection and RPC URL."
    // Instead, it must successfully broadcast and return a valid transaction hash!
    assert!(
        result.is_ok(),
        "sentient V3 swap must not fail with fresh swap quote failed: {result:?}"
    );
    let tx_hash = result.unwrap();
    assert!(
        tx_hash.starts_with("0x") && tx_hash.len() == 66,
        "expected valid tx hash, got {tx_hash}"
    );
}

#[test]
fn test_refused_swap_leaves_spent_unchanged_and_allows_retry() {
    let anvil = Anvil::start();
    let (wallet, _dir) = setup_sentient_wallet(&anvil);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
    let profile_dir = vaughan_agent::paths::profile_dir(wallet.path());

    let router = address!("DaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7");
    let token_in = address!("70499adebb11efd915e3b69e700c331778628707");
    let token_out = address!("29bab93456c0e97ee931c1554c7c215480aa7766");
    let amount_in = U256::from(500_000_000_000_000_000u64);

    let prop_id = "prop_refused_retryable";

    // Proposal with invalid calldata selector will be refused by the V2 gate
    let bad_prop = TxProposal::new(
        prop_id,
        ProposalType::DexSwap {
            router,
            path: vec![token_in, token_out],
            amount_in,
            min_amount_out: U256::from(100),
            dex_kind: DexSwapKind::V2,
        },
        router,
        amount_in,
        Bytes::from(vec![0x12, 0x34, 0x56, 0x78]),
        250_000,
        true,
        "refused swap",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let snap_before = SpendLedger::snapshot(&profile_dir).unwrap();
    let spent_before = snap_before
        .assets
        .get("943:native")
        .map(|a| a.spent)
        .unwrap_or(U256::ZERO);

    let err = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &bad_prop).unwrap_err();
    assert!(err
        .to_string()
        .contains("does not belong to the audited V2 swap family"));

    // Spent MUST remain unchanged after refusal
    let snap_after = SpendLedger::snapshot(&profile_dir).unwrap();
    let spent_after = snap_after
        .assets
        .get("943:native")
        .map(|a| a.spent)
        .unwrap_or(U256::ZERO);
    assert_eq!(
        spent_before, spent_after,
        "refused swap must not consume spend cap"
    );
    assert!(!snap_after.recent.iter().any(|r| r.key == prop_id));

    // Retrying with the same proposal_id MUST NOT fail with spend_already_reserved
    let err_retry = gate_sentient_proposal(&wallet, rt.handle(), &breaker, &bad_prop).unwrap_err();
    assert!(
        err_retry
            .to_string()
            .contains("does not belong to the audited V2 swap family"),
        "retry should fail with the policy error, not spend_already_reserved: {err_retry}"
    );
    assert!(!err_retry.to_string().contains("spend_already_reserved"));
}
