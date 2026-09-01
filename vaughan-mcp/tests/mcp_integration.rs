//! Integration tests for the MCP proposal queue (no live TUI required).

use alloy::primitives::{address, U256};
use serde_json::json;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::TempDir;
use vaughan_agent::tools::{default_assist_registry, ToolContext};
use vaughan_core::core::proposal::{ProposalQueue, TxProposal};

struct AnvilGuard {
    child: Child,
    rpc_url: String,
}

impl AnvilGuard {
    fn spawn(port: u16) -> Self {
        let child = Command::new("anvil")
            .args(["-p", &port.to_string(), "--silent"])
            .spawn()
            .expect("anvil required for MCP integration tests");
        std::thread::sleep(Duration::from_millis(400));
        Self {
            child,
            rpc_url: format!("http://127.0.0.1:{port}"),
        }
    }
}

impl Drop for AnvilGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[tokio::test]
async fn mcp_propose_transfer_queues_and_roundtrips() {
    let anvil = AnvilGuard::spawn(8760);
    let dir = TempDir::new().unwrap();
    let secret = b"test-session-token-bytes!!";

    let registry = default_assist_registry();
    let sender = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    let recipient = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(sender),
            profile_dir: None,
    };

    let raw = registry
        .execute(
            "propose_transfer",
            json!({
                "recipient": format!("{recipient:#x}"),
                "amount": "1000000000000000000",
                "explanation": "MCP test transfer"
            }),
            &context,
        )
        .await
        .unwrap();

    let proposal: TxProposal = serde_json::from_value(raw).unwrap();
    assert_eq!(proposal.chain_id, 31337);

    let queue = ProposalQueue::new(dir.path());
    let queued = queue
        .enqueue(proposal.clone(), "test", secret)
        .expect("enqueue");

    let loaded = queue
        .get_pending(&queued.proposal.proposal_id, secret)
        .expect("load pending");
    assert_eq!(loaded.proposal.explanation, "MCP test transfer");
    assert_eq!(
        loaded.proposal.value_wei,
        U256::from(1_000_000_000_000_000_000u64)
    );
}

#[test]
fn mcp_tool_definitions_include_banned_absence() {
    let registry = default_assist_registry();
    let defs = registry.definitions();
    let names: Vec<_> = defs.iter().map(|d| d.name.as_str()).collect();
    for banned in ["sign_transaction", "export_key", "unlock", "broadcast_tx"] {
        assert!(
            !names.iter().any(|n| n.contains(banned)),
            "banned tool leaked: {banned}"
        );
    }
    assert!(names.contains(&"propose_transfer"));
    assert!(names.contains(&"get_balance"));
    assert!(names.contains(&"quote_swap"));
    assert!(names.contains(&"propose_agg_swap"));
    assert!(names.contains(&"get_v3_pool"));
    assert!(names.contains(&"quote_v3_swap"));
    assert!(names.contains(&"propose_v3_swap"));
    assert!(names.contains(&"propose_v3_mint"));
    assert!(names.contains(&"propose_v3_create_pool"));
    assert!(names.contains(&"propose_v3_initialize_pool"));
    assert!(names.contains(&"propose_wrap"));
    assert!(names.contains(&"propose_unwrap"));
    assert!(names.contains(&"propose_revoke"));
    assert!(names.contains(&"list_allowances"));
    assert!(names.contains(&"list_v3_positions"));
}
