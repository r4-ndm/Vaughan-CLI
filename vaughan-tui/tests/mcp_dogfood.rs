//! Dogfood coverage for MCP approval paths (reject queue + re-sim at approve).
//!
//! These replace a manual Cursor↔TUI session for the security-critical bits
//! that docs cannot catch: deny must land in `rejected/`, and a proposal that
//! fails `eth_call` must not sign.
//!
//! Mapped to [`docs/mcp-threat-model.md`](../../docs/mcp-threat-model.md).

mod common;

use alloy::primitives::{Address, Bytes, U256};
use common::{funded_wallet, Anvil};
use vaughan_core::core::proposal::{ProposalQueue, ProposalType, TxProposal, PROPOSAL_TTL_SECS};
use vaughan_core::core::WalletState;
use vaughan_tui::provider::{execute_approval, ApprovalKind};

fn mcp_proposal_kind(proposal: TxProposal) -> ApprovalKind {
    let proposal_id = proposal.proposal_id.clone();
    ApprovalKind::McpProposal {
        proposal_id,
        source: "cursor".into(),
        proposal: Box::new(proposal),
    }
}

#[tokio::test]
async fn mcp_user_reject_lands_in_rejected_queue() {
    let dir = tempfile::tempdir().unwrap();
    let secret = b"dogfood-session-secret-32b!!!!!";
    let proposal = TxProposal::new(
        "prop_reject_dogfood",
        ProposalType::NativeTransfer {
            to: Address::ZERO,
            amount_wei: U256::from(1u64),
        },
        Address::ZERO,
        U256::from(1u64),
        Bytes::new(),
        21_000,
        true,
        "dogfood reject",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));

    let queue = ProposalQueue::new(dir.path());
    queue
        .enqueue(proposal.clone(), "cursor", secret)
        .expect("enqueue");

    queue
        .mark_rejected(&proposal.proposal_id, "user rejected", secret)
        .expect("mark_rejected");

    assert!(
        queue.get_pending(&proposal.proposal_id, secret).is_err(),
        "rejected proposal must leave pending/"
    );
    let rejected = dir
        .path()
        .join("proposals/rejected")
        .join(format!("{}.json", proposal.proposal_id));
    assert!(
        rejected.is_file(),
        "expected rejected file at {}",
        rejected.display()
    );
    let body = std::fs::read_to_string(&rejected).unwrap();
    assert!(body.contains("\"status\": \"rejected\"") || body.contains("\"status\":\"rejected\""));
    assert!(body.contains("user rejected"));
}

#[tokio::test]
async fn mcp_resim_blocks_insufficient_funds_before_sign() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    wallet.set_rpc_override(anvil.url());

    // Far more than the anvil-funded balance → eth_call reverts at approve.
    let huge = U256::from_str_radix("1000000000000000000000000000000", 10).unwrap();
    let recipient: Address = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
        .parse()
        .unwrap();
    let proposal = TxProposal::new(
        "prop_resim_fail",
        ProposalType::NativeTransfer {
            to: recipient,
            amount_wei: huge,
        },
        recipient,
        huge,
        Bytes::new(),
        21_000,
        true, // agent claimed success — wallet must not trust this
        "dogfood overspend",
    )
    .with_chain(943, None);

    let kind = mcp_proposal_kind(proposal);

    let err = execute_approval(&kind, &mut wallet)
        .await
        .expect_err("overspend must fail re-sim");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("simulation") || msg.contains("revert") || msg.contains("insufficient"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn mcp_chain_mismatch_blocks_sign() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    wallet.set_rpc_override(anvil.url());

    let recipient: Address = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
        .parse()
        .unwrap();
    let proposal = TxProposal::new(
        "prop_chain_mismatch",
        ProposalType::NativeTransfer {
            to: recipient,
            amount_wei: U256::from(1u64),
        },
        recipient,
        U256::from(1u64),
        Bytes::new(),
        21_000,
        true,
        "agent lied about chain",
    )
    .with_chain(369, Some("pulsechain".into()));

    let err = execute_approval(&mcp_proposal_kind(proposal), &mut wallet)
        .await
        .expect_err("wrong chain must not sign");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("network mismatch") || msg.contains("369"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn mcp_locked_wallet_blocks_sign() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    wallet.set_rpc_override(anvil.url());
    wallet.lock();

    let recipient: Address = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
        .parse()
        .unwrap();
    let proposal = TxProposal::new(
        "prop_locked",
        ProposalType::NativeTransfer {
            to: recipient,
            amount_wei: U256::from(1u64),
        },
        recipient,
        U256::from(1u64),
        Bytes::new(),
        21_000,
        true,
        "locked wallet",
    )
    .with_chain(943, None);

    let err = execute_approval(&mcp_proposal_kind(proposal), &mut wallet)
        .await
        .expect_err("locked wallet must not sign");
    assert!(err.to_string().to_lowercase().contains("locked"));
}

#[tokio::test]
async fn mcp_expired_proposal_blocks_sign() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);

    let recipient: Address = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
        .parse()
        .unwrap();
    let mut proposal = TxProposal::new(
        "prop_expired",
        ProposalType::NativeTransfer {
            to: recipient,
            amount_wei: U256::from(1u64),
        },
        recipient,
        U256::from(1u64),
        Bytes::new(),
        21_000,
        true,
        "stale proposal",
    )
    .with_chain(943, None);
    proposal.created_at_unix = proposal
        .created_at_unix
        .saturating_sub(PROPOSAL_TTL_SECS + 60);

    let err = execute_approval(&mcp_proposal_kind(proposal), &mut wallet)
        .await
        .expect_err("expired proposal must not sign");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("expired") || msg.contains("simulation"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn mcp_fee_spike_blocks_sign() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    wallet.set_rpc_override(anvil.url());

    let recipient: Address = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
        .parse()
        .unwrap();
    let amount = U256::from(1_000u64);
    let mut proposal = TxProposal::new(
        "prop_fee_spike",
        ProposalType::NativeTransfer {
            to: recipient,
            amount_wei: amount,
        },
        recipient,
        amount,
        Bytes::new(),
        21_000,
        true,
        "dogfood fee spike",
    )
    .with_chain(943, None);
    // Agent stamped a trivial fee; fresh estimate at approve must exceed 110%.
    proposal.estimated_fee_wei = Some(U256::from(1u64));

    let err = execute_approval(&mcp_proposal_kind(proposal), &mut wallet)
        .await
        .expect_err("fee spike must block sign");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("fee") && msg.contains("10%"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn lp_deploy_step_skips_batch7702_decode_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = WalletState::load(dir.path().join("w.json")).unwrap();

    let calldata = hex::decode(
        "a167129500000000000000000000000033df366093ef8ac488e5be40e7ee2eeac2142770000000000000000000000000fc413180d3624349d111fd98ee76bc08a25bc6550000000000000000000000000000000000000000000000000000000000004e20",
    )
    .unwrap();
    let factory: Address = "0x297BeFB564d3Bba2D1913613B84Fb743C259C6cf"
        .parse()
        .unwrap();
    let proposal = TxProposal::new(
        "lp-createPool-route",
        ProposalType::LpDeployStep {
            job_id: "lp_test".into(),
            step_label: "createPool".into(),
        },
        factory,
        U256::ZERO,
        Bytes::from(calldata),
        500_000,
        true,
        "route test",
    )
    .with_chain(943, None);

    let err = execute_approval(&mcp_proposal_kind(proposal), &mut wallet)
        .await
        .expect_err("locked wallet must not sign");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("locked"),
        "LP deploy must use EVM send path (fail on locked wallet), not batch7702 decode: {err}"
    );
    assert!(!msg.contains("batch7702") && !msg.contains("decode"));
}

#[tokio::test]
async fn lp_deploy_step_applies_fee_override_to_evm_tx() {
    use vaughan_core::chains::{ChainTransaction, Fee, FeeDetails};
    use vaughan_core::core::proposal::apply_proposal;
    use vaughan_core::core::transaction::TransactionService;

    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);

    let calldata = hex::decode(
        "095ea7b3000000000000000000000000fc413180d3624349d111fd98ee76bc08a25bc6550000000000000000000000000000000000000000000000000000000000004e20",
    )
    .unwrap();
    let npm: Address = "0x33df366093ef8ac488e5be40e7ee2eeac2142770"
        .parse()
        .unwrap();
    let proposal = TxProposal::new(
        "lp-fee-override",
        ProposalType::LpDeployStep {
            job_id: "lp_fee_test".into(),
            step_label: "approve token0".into(),
        },
        npm,
        U256::ZERO,
        Bytes::from(calldata),
        100_000,
        true,
        "fee override dogfood",
    )
    .with_chain(943, None);

    let evm = apply_proposal(&wallet, &proposal).expect("apply_proposal");
    let override_max = 88_000_000_000u128;
    let override_tip = 7_000_000_000u128;
    let fee = Fee {
        total: "test".into(),
        currency: "tPLS".into(),
        details: FeeDetails::Evm {
            gas_limit: evm.gas_limit.unwrap_or(100_000),
            max_fee_per_gas: Some(override_max.to_string()),
            max_priority_fee_per_gas: Some(override_tip.to_string()),
        },
    };

    // Mirrors execute_approval_with_fee LP-deploy branch (fee_override path).
    let mut chain_tx = ChainTransaction::Evm(evm);
    TransactionService::new()
        .apply_fee(&mut chain_tx, &fee)
        .expect("apply_fee");
    let ChainTransaction::Evm(signed_shape) = chain_tx else {
        panic!("expected EVM tx");
    };

    assert_eq!(
        signed_shape.max_fee_per_gas.as_deref(),
        Some(override_max.to_string().as_str())
    );
    assert_eq!(
        signed_shape.max_priority_fee_per_gas.as_deref(),
        Some(override_tip.to_string().as_str())
    );
}
