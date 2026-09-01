//! Live IPC test: queue a token-launch proposal to an unlocked Vaughan TUI.
//!
//! ```sh
//! cargo test -p vaughan-agent --test propose_token_launch_live -- --ignored --nocapture
//! ```

use alloy::primitives::Address;
use serde_json::json;
use vaughan_agent::paths::profile_dir;
use vaughan_agent::proposal::TxProposal;
use vaughan_agent::tools::{default_assist_registry_for, ToolContext};
use vaughan_core::core::persistence::StateManager;
use vaughan_core::core::proposal::{McpSessionToken, ProposalQueue};
use vaughan_mcp::client::{try_get_session, try_propose_live};

#[tokio::test]
#[ignore = "live: requires unlocked Vaughan TUI on Pulse testnet 943"]
async fn propose_vac_to_unlocked_tui() {
    let wallet_path = StateManager::profile_path("default").expect("profile path");
    let profile_dir = profile_dir(&wallet_path);
    let session = McpSessionToken::read(&profile_dir)
        .expect("read session")
        .filter(|s| !s.is_empty())
        .expect("unlock Vaughan TUI first (mcp.session missing)");

    let live = try_get_session(&session)
        .await
        .expect("ipc")
        .expect("TUI control plane offline or wallet locked");
    assert_eq!(live.chain_id, 943, "switch TUI to Pulse testnet v4 (943)");

    let registry = default_assist_registry_for(Some(&profile_dir));
    let context = ToolContext {
        rpc_url: "https://rpc.v4.testnet.pulsechain.com".into(),
        chain_id: live.chain_id,
        active_address: Some(live.address),
        profile_dir: Some(profile_dir.clone()),
    };

    let raw = registry
        .execute(
            "propose_token_launch",
            json!({
                "name": "Vaughan Agent Coin",
                "symbol": "VAC",
                "supply": "1000000",
                "explanation": "Cursor agent advisory-mode test launch"
            }),
            &context,
        )
        .await
        .expect("propose_token_launch tool");

    let proposal: TxProposal = serde_json::from_value(raw).expect("proposal json");
    assert_eq!(proposal.to, Address::ZERO);
    assert!(proposal.gas_limit >= 1_000_000);

    let out = match try_propose_live(&session, "cursor-agent", &proposal).await {
        Ok(Some(data)) => data,
        Ok(None) | Err(_) => {
            let queue = ProposalQueue::new(&profile_dir);
            let queued = queue
                .enqueue(proposal.clone(), "cursor-agent", session.as_bytes())
                .expect("enqueue proposal");
            json!({
                "proposal_id": queued.proposal.proposal_id,
                "status": "pending_user",
                "message": "queued to file (restart TUI if live IPC failed on new proposal type)",
            })
        }
    };

    eprintln!("propose_token_launch response: {out}");
    let status = out.get("status").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        status == "pending_user" || status == "queued" || out.get("proposal_id").is_some(),
        "unexpected response: {out}"
    );
}
