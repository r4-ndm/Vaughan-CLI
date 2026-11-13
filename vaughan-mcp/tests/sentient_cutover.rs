//! Integration tests for S2 MCP cutover to the isolated signer.
//!
//! Verifies:
//! 1. Every untyped tool returns `intent_untyped` naming the adviser path and never touches the signer.
//! 2. A signer refusal (e.g. spend_cap_exceeded, destination_not_allowed) surfaces unchanged through MCP.
//! 3. `propose_transfer` via MCP produces a signed/broadcast transaction through the signer.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use vaughan_mcp::{McpContext, McpDispatcher};

static TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn test_untyped_tools_return_intent_untyped_and_do_not_touch_signer() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::set_var("VAUGHAN_ASSIST_UNLOCK_BYPASS", "1");
    let dir = tempdir().unwrap();
    let sentient_dir = dir.path().join("sentient");
    std::fs::create_dir_all(&sentient_dir).unwrap();

    let socket_path = sentient_dir.join("test_signerd.sock");
    // Signer socket that counts any connections — untyped tools must NEVER connect.
    let listener = UnixListener::bind(&socket_path).unwrap();
    let connection_count = Arc::new(AtomicUsize::new(0));
    let cc = connection_count.clone();

    let server_task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            cc.fetch_add(1, Ordering::SeqCst);
            let _ = stream.write_all(b"{\"ok\":true}\n").await;
        }
    });

    std::env::set_var("VAUGHAN_SIGNER_SOCKET", &socket_path);

    let dispatcher = McpDispatcher::for_profile_dir(sentient_dir);
    let ctx = McpContext {
        profile: "sentient".into(),
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 943,
        network_id: "pulsechain-testnet-v4".into(),
        active_address: Some(alloy::primitives::Address::repeat_byte(0x11)),
        is_testnet: true,
        source: "test".into(),
    };

    let untyped_tools = [
        (
            "propose_contract_call",
            json!({
                "target": "0x1111111111111111111111111111111111111111",
                "calldata": "0x12345678",
                "explanation": "call raw"
            }),
        ),
        (
            "propose_lp_deploy",
            json!({
                "token_a": "0x1111111111111111111111111111111111111111",
                "token_b": "0x2222222222222222222222222222222222222222",
                "explanation": "lp"
            }),
        ),
        (
            "propose_v3_mint",
            json!({
                "token0": "0x1111111111111111111111111111111111111111",
                "token1": "0x2222222222222222222222222222222222222222",
                "fee": 500,
                "tick_lower": -100,
                "tick_upper": 100,
                "amount0_desired": "1000",
                "amount1_desired": "1000",
                "explanation": "mint"
            }),
        ),
        (
            "propose_token_launch",
            json!({
                "name": "TestToken",
                "symbol": "TST",
                "supply": "1000000",
                "explanation": "launch"
            }),
        ),
        (
            "propose_bridge",
            json!({
                "token": "0x1111111111111111111111111111111111111111",
                "amount": "1000",
                "explanation": "bridge"
            }),
        ),
        (
            "propose_hex_stake",
            json!({
                "amount": "1000",
                "days": 30,
                "explanation": "stake"
            }),
        ),
        (
            "propose_policy",
            json!({
                "explanation": "policy"
            }),
        ),
    ];

    for (tool_name, args) in untyped_tools {
        let err = dispatcher
            .call_tool(tool_name, args, &ctx)
            .await
            .unwrap_err();
        assert!(
            err.starts_with("intent_untyped:"),
            "expected error starting with 'intent_untyped:' for {tool_name}, got: {err}"
        );
        assert!(
            err.contains("requires human approval via an adviser profile"),
            "expected adviser profile mention for {tool_name}, got: {err}"
        );
    }

    // Verify signer socket was NEVER contacted by any untyped tool
    assert_eq!(
        connection_count.load(Ordering::SeqCst),
        0,
        "untyped tools must never touch the signer socket"
    );

    std::env::remove_var("VAUGHAN_SIGNER_SOCKET");
    std::env::remove_var("VAUGHAN_ASSIST_UNLOCK_BYPASS");
    server_task.abort();
}

#[tokio::test]
async fn test_signer_refusal_surfaces_unchanged_through_mcp() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::set_var("VAUGHAN_ASSIST_UNLOCK_BYPASS", "1");
    let dir = tempdir().unwrap();
    let sentient_dir = dir.path().join("sentient");
    std::fs::create_dir_all(&sentient_dir).unwrap();

    let socket_path = sentient_dir.join("refusal_signerd.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();

    let server_task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut buf = BufReader::new(reader);
                let mut line = String::new();
                if buf.read_line(&mut line).await.is_ok() {
                    let resp = json!({
                        "ok": false,
                        "code": "spend_cap_exceeded",
                        "message": "spend_cap_exceeded: 50000000000000000 wei exceeds daily cap"
                    });
                    let _ = writer.write_all((resp.to_string() + "\n").as_bytes()).await;
                }
            });
        }
    });

    std::env::set_var("VAUGHAN_SIGNER_SOCKET", &socket_path);

    let dispatcher = McpDispatcher::for_profile_dir(sentient_dir);
    let ctx = McpContext {
        profile: "sentient".into(),
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 943,
        network_id: "pulsechain-testnet-v4".into(),
        active_address: Some(alloy::primitives::Address::repeat_byte(0x11)),
        is_testnet: true,
        source: "test".into(),
    };

    let err = dispatcher
        .call_tool(
            "propose_transfer",
            json!({
                "recipient": "0x2222222222222222222222222222222222222222",
                "amount": "50000000000000000",
                "explanation": "large transfer"
            }),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(
        err.starts_with("spend_cap_exceeded:"),
        "expected error starting with spend_cap_exceeded:, got: {err}"
    );
    assert!(err.contains("exceeds daily cap"));

    std::env::remove_var("VAUGHAN_SIGNER_SOCKET");
    std::env::remove_var("VAUGHAN_ASSIST_UNLOCK_BYPASS");
    server_task.abort();
}

#[tokio::test]
async fn test_propose_transfer_via_mcp_produces_signed_broadcast_through_signer() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::set_var("VAUGHAN_ASSIST_UNLOCK_BYPASS", "1");
    let dir = tempdir().unwrap();
    let sentient_dir = dir.path().join("sentient");
    std::fs::create_dir_all(&sentient_dir).unwrap();

    let socket_path = sentient_dir.join("valid_signerd.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();

    let recipient_addr = "0x2222222222222222222222222222222222222222";
    let expected_hash = "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";

    let server_task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut buf = BufReader::new(reader);
                let mut line = String::new();
                if buf.read_line(&mut line).await.is_ok() {
                    let req: serde_json::Value = serde_json::from_str(&line).unwrap();
                    if req["op"] == "sign" {
                        let resp = json!({
                            "ok": true,
                            "tx_hash": expected_hash,
                        });
                        let _ = writer.write_all((resp.to_string() + "\n").as_bytes()).await;
                    }
                }
            });
        }
    });

    std::env::set_var("VAUGHAN_SIGNER_SOCKET", &socket_path);

    let dispatcher = McpDispatcher::for_profile_dir(sentient_dir);
    let ctx = McpContext {
        profile: "sentient".into(),
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 943,
        network_id: "pulsechain-testnet-v4".into(),
        active_address: Some(alloy::primitives::Address::repeat_byte(0x11)),
        is_testnet: true,
        source: "test".into(),
    };

    let result = dispatcher
        .call_tool(
            "propose_transfer",
            json!({
                "recipient": recipient_addr,
                "amount": "1000000000",
                "explanation": "valid transfer"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(result["status"], "approved");
    assert_eq!(result["tx_hash"], expected_hash);

    std::env::remove_var("VAUGHAN_SIGNER_SOCKET");
    std::env::remove_var("VAUGHAN_ASSIST_UNLOCK_BYPASS");
    server_task.abort();
}
