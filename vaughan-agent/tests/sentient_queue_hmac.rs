//! Integration test: sentient queue HMAC separation from session token.
//!
//! Asserts that a proposal in the queue signed with the legacy loopback session
//! token fails verification, returning an error whose reason names the key change.

use alloy::primitives::{address, Bytes, U256};
use tempfile::tempdir;
use vaughan_agent::sentient::{
    load_policy, load_signer_key, sentient_queue_hmac_key, verify_sentient_queue_entry,
};
use vaughan_core::core::proposal::{McpSessionToken, ProposalQueue, ProposalType, TxProposal};

fn sample_proposal(id: &str) -> TxProposal {
    TxProposal::new(
        id,
        ProposalType::NativeTransfer {
            to: address!("0000000000000000000000000000000000000001"),
            amount_wei: U256::from(1_000_000_000_000_000_000u64),
        },
        address!("0000000000000000000000000000000000000001"),
        U256::from(1_000_000_000_000_000_000u64),
        Bytes::new(),
        21000,
        true,
        "test transfer",
    )
}

#[test]
fn test_queue_entry_signed_with_old_session_token_refused() {
    let dir = tempdir().unwrap();

    // 1. Preset apply creates 0600 policy and 0600 signer.key
    vaughan_agent::apply_preset("balanced", dir.path()).unwrap();
    assert!(load_policy(dir.path()).is_ok());
    assert!(load_signer_key(dir.path()).is_ok());

    // 2. Loopback session token is written to mcp.session
    let token = McpSessionToken::generate();
    token.write(dir.path()).unwrap();

    let queue = ProposalQueue::new(dir.path());
    let proposal = sample_proposal("prop_legacy_token");

    // 3. Enqueue using the OLD session token
    let queued = queue
        .enqueue(proposal.clone(), "rogue_script", token.as_str().as_bytes())
        .expect("enqueue succeeds with session token bytes");

    // 4. Verification for sentient queue MUST fail closed
    let result = verify_sentient_queue_entry(&queued, dir.path());
    assert!(result.is_err(), "must refuse legacy session token HMAC");

    let err = result.unwrap_err();
    assert_eq!(err.code(), "hmac_invalid");
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("legacy session token")
            || (err_msg.contains("signer key") && err_msg.contains("session token")),
        "reason must name key change, got: {err_msg}"
    );

    // 5. Also verify that get_pending_sentient on the queue refuses and names key change
    let queue_secret = sentient_queue_hmac_key(dir.path()).unwrap();
    let legacy_token_bytes = token.as_str().as_bytes();
    let pending_err = queue
        .get_pending_sentient(
            &proposal.proposal_id,
            &queue_secret,
            Some(legacy_token_bytes),
        )
        .unwrap_err();
    assert_eq!(pending_err.code(), "hmac_invalid");
    let pending_msg = pending_err.to_string();
    assert!(
        pending_msg.contains("legacy session token") && pending_msg.contains("signer key"),
        "reason must name key change, got: {pending_msg}"
    );

    // 6. Proposal enqueued with the derived sentient queue key verifies cleanly
    let proposal_valid = sample_proposal("prop_valid_signer_key");
    let queued_valid = queue
        .enqueue(proposal_valid.clone(), "agent", &queue_secret)
        .expect("enqueue with derived queue key");

    assert!(verify_sentient_queue_entry(&queued_valid, dir.path()).is_ok());
    assert!(queue
        .get_pending_sentient(
            &proposal_valid.proposal_id,
            &queue_secret,
            Some(legacy_token_bytes),
        )
        .is_ok());
}
