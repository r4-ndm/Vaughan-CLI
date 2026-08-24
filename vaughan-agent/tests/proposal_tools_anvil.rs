//! Deterministic proposal tool tests against a local Anvil node.

use alloy::primitives::address;
use serde_json::json;
use std::process::{Child, Command};
use std::time::Duration;

use vaughan_agent::proposal::TxProposal;
use vaughan_agent::tools::{default_assist_registry, ToolContext};

struct AnvilGuard {
    child: Child,
    rpc_url: String,
}

impl AnvilGuard {
    fn spawn(port: u16) -> Self {
        let child = Command::new("anvil")
            .args(["-p", &port.to_string(), "--silent"])
            .spawn()
            .expect("Failed to start Anvil.");

        std::thread::sleep(Duration::from_millis(400));
        let rpc_url = format!("http://127.0.0.1:{}", port);
        Self { child, rpc_url }
    }
}

impl Drop for AnvilGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[tokio::test]
async fn test_assist_registry_and_proposals_with_anvil() {
    let anvil = AnvilGuard::spawn(8556);
    let registry = default_assist_registry();
    let defs = registry.definitions();

    assert_eq!(defs.len(), 9);

    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
    };

    // 1. Propose Native Transfer
    let prop_raw = registry
        .execute(
            "propose_transfer",
            json!({
                "recipient": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
                "amount": "1000000000000000000",
                "explanation": "Sending 1 ETH to second test account"
            }),
            &context,
        )
        .await
        .unwrap();

    let prop: TxProposal = serde_json::from_value(prop_raw).unwrap();
    assert_eq!(
        prop.to,
        address!("70997970C51812dc3A010C7d01b50e0d17dc79C8")
    );
    assert_eq!(prop.value_wei.to_string(), "1000000000000000000");
    assert!(prop.simulation_success);
    assert_eq!(prop.explanation, "Sending 1 ETH to second test account");

    // 2. Propose Batch 7702
    let batch_raw = registry
        .execute(
            "propose_batch_7702",
            json!({
                "calls": [
                    { "to": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8", "value_wei": "500000000000000000", "data": "0x" },
                    { "to": "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC", "value_wei": "500000000000000000", "data": "0x" }
                ],
                "explanation": "Batch distribution to 2 recipients"
            }),
            &context,
        )
        .await
        .unwrap();

    let batch_prop: TxProposal = serde_json::from_value(batch_raw).unwrap();
    assert_eq!(batch_prop.value_wei.to_string(), "1000000000000000000");
    assert!(!batch_prop.calldata.is_empty());

    // 3. Propose Contract Call
    let call_raw = registry
        .execute(
            "propose_contract_call",
            json!({
                "to": "0x1111111111111111111111111111111111111111",
                "calldata": "0xd0e30db0",
                "value_wei": "1000000000000000000",
                "function_name": "deposit",
                "explanation": "Wrap 1 ETH to WETH"
            }),
            &context,
        )
        .await
        .unwrap();

    let call_prop: TxProposal = serde_json::from_value(call_raw).unwrap();
    assert_eq!(
        call_prop.to,
        address!("1111111111111111111111111111111111111111")
    );
    assert_eq!(call_prop.calldata.to_string(), "0xd0e30db0");
}
