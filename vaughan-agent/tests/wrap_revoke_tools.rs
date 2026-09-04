//! Unit coverage for wrap / revoke / mint tool registration and wrap proposal shape.

use alloy::primitives::{address, U256};
use serde_json::json;
use vaughan_agent::proposal::TxProposal;
use vaughan_agent::tools::{default_assist_registry, ToolContext};

#[tokio::test]
async fn propose_wrap_builds_deposit_proposal() {
    let registry = default_assist_registry();
    let ctx = ToolContext {
        rpc_url: "http://127.0.0.1:1".into(),
        chain_id: 943,
        active_address: Some(address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
        profile_dir: None,
    };
    let raw = registry
        .execute(
            "propose_wrap",
            json!({
                "amount": "1000000000000000000",
                "explanation": "wrap one tPLS"
            }),
            &ctx,
        )
        .await
        .expect("propose_wrap");
    let proposal: TxProposal = serde_json::from_value(raw).unwrap();
    assert_eq!(proposal.value_wei, U256::from(10u64.pow(18)));
    assert_eq!(
        format!("{:#x}", proposal.to).to_lowercase(),
        "0x70499adebb11efd915e3b69e700c331778628707"
    );
}

#[tokio::test]
async fn propose_revoke_builds_approve_zero() {
    let registry = default_assist_registry();
    let ctx = ToolContext {
        rpc_url: "http://127.0.0.1:1".into(),
        chain_id: 943,
        active_address: Some(address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
        profile_dir: None,
    };
    let token = "0x70499adEBB11Efd915E3b69E700c331778628707";
    let spender = "0xfC656c95eCd418536844FeeaA46949bb9365BEaF";
    let raw = registry
        .execute(
            "propose_revoke",
            json!({
                "token": token,
                "spender": spender,
                "explanation": "revoke router"
            }),
            &ctx,
        )
        .await
        .expect("propose_revoke");
    let proposal: TxProposal = serde_json::from_value(raw).unwrap();
    assert_eq!(proposal.value_wei, U256::ZERO);
    assert!(proposal.calldata.len() >= 4);
}
