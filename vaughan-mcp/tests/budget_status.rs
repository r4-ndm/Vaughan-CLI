//! Tests for the `budget_status` MCP tool.

use serde_json::json;
use tempfile::tempdir;
use vaughan_mcp::{McpContext, McpDispatcher};

fn write_mode_0600(path: &std::path::Path, content: &str) {
    std::fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

#[tokio::test]
async fn test_budget_status_adviser_profile_shape() {
    let dir = tempdir().unwrap();
    let adviser_dir = dir.path().join("default");
    std::fs::create_dir_all(&adviser_dir).unwrap();
    let dispatcher = McpDispatcher::for_profile_dir(adviser_dir);

    let ctx = McpContext {
        profile: "default".into(),
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 369,
        network_id: "pulsechain".into(),
        active_address: None,
        is_testnet: false,
        source: "test".into(),
    };

    let result = dispatcher
        .call_tool("budget_status", json!({}), &ctx)
        .await
        .unwrap();

    assert_eq!(result["profile"], "default");
    assert_eq!(result["enforcement"], "n/a");
    assert_eq!(result["assets"], json!([]));
    assert_eq!(
        result["note"],
        "adviser profile — proposals need human approval"
    );
}

#[tokio::test]
async fn test_budget_status_sentient_values_and_arithmetic() {
    let dir = tempdir().unwrap();
    let sentient_dir = dir.path().join("sentient");
    std::fs::create_dir_all(&sentient_dir).unwrap();

    // Default policy: 20% cap
    let policy_toml = r#"
enforcement = "enforced"
daily_cap_pct = 20
allow_transfer_to = ["0x1111111111111111111111111111111111111111"]
"#;
    write_mode_0600(&sentient_dir.join("sentient-policy.toml"), policy_toml);

    // Ledger with 1 ETH snapshot and 0.1 ETH spent
    let ledger_json = r#"{
  "version": 1,
  "window_start_unix": 1729913600,
  "window_seconds": 86400,
  "assets": {
    "369:native": {
      "snapshot": "1000000000000000000",
      "spent": "100000000000000000"
    }
  },
  "recent": []
}"#;
    write_mode_0600(&sentient_dir.join("sentient-spend.json"), ledger_json);

    let dispatcher = McpDispatcher::for_profile_dir(sentient_dir.clone());
    let ctx = McpContext {
        profile: "sentient".into(),
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 369,
        network_id: "pulsechain".into(),
        active_address: None,
        is_testnet: false,
        source: "test".into(),
    };

    let result = dispatcher
        .call_tool("budget_status", json!({}), &ctx)
        .await
        .unwrap();

    assert_eq!(result["profile"], "sentient");
    assert_eq!(result["chain_id"], 369);
    assert_eq!(result["enforcement"], "enforced");
    assert_eq!(result["daily_cap_pct"], 20);
    assert_eq!(result["window_reset_unix"], 1730000000); // 1729913600 + 86400
    assert_eq!(
        result["ledger_path"],
        sentient_dir
            .join("sentient-spend.json")
            .display()
            .to_string()
    );

    let assets = result["assets"].as_array().expect("assets array");
    assert_eq!(assets.len(), 1);
    let asset = &assets[0];
    assert_eq!(asset["asset"], "369:native");
    assert_eq!(asset["snapshot"], "1000000000000000000");
    assert_eq!(asset["spent"], "100000000000000000");
    assert_eq!(asset["cap"], "200000000000000000"); // 20% of 1 ETH
    assert_eq!(asset["remaining"], "100000000000000000"); // 0.2 ETH - 0.1 ETH
    assert_eq!(
        result["allow_transfer_to"],
        json!(["0x1111111111111111111111111111111111111111"])
    );
}

#[tokio::test]
async fn test_budget_status_sentient_empty_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let sentient_dir = dir.path().join("sentient");
    std::fs::create_dir_all(&sentient_dir).unwrap();

    let policy_toml = r#"
enforcement = "enforced"
daily_cap_pct = 20
allow_transfer_to = []
"#;
    write_mode_0600(&sentient_dir.join("sentient-policy.toml"), policy_toml);

    let dispatcher = McpDispatcher::for_profile_dir(sentient_dir);
    let ctx = McpContext {
        profile: "sentient".into(),
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 369,
        network_id: "pulsechain".into(),
        active_address: None,
        is_testnet: false,
        source: "test".into(),
    };

    let result = dispatcher
        .call_tool("budget_status", json!({}), &ctx)
        .await
        .unwrap();

    assert_eq!(result["profile"], "sentient");
    assert_eq!(result["assets"], json!([]));
    assert_eq!(result["note"], "no spend recorded this window");
}

#[tokio::test]
async fn test_budget_status_corrupt_ledger_errors() {
    let dir = tempfile::tempdir().unwrap();
    let sentient_dir = dir.path().join("sentient");
    std::fs::create_dir_all(&sentient_dir).unwrap();

    let policy_toml = r#"
enforcement = "enforced"
daily_cap_pct = 20
"#;
    write_mode_0600(&sentient_dir.join("sentient-policy.toml"), policy_toml);

    // Write corrupted JSON to sentient-spend.json
    write_mode_0600(
        &sentient_dir.join("sentient-spend.json"),
        "this is not valid json {{{[[",
    );

    let dispatcher = McpDispatcher::for_profile_dir(sentient_dir);
    let ctx = McpContext {
        profile: "sentient".into(),
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 369,
        network_id: "pulsechain".into(),
        active_address: None,
        is_testnet: false,
        source: "test".into(),
    };

    let err = dispatcher
        .call_tool("budget_status", json!({}), &ctx)
        .await
        .unwrap_err();

    assert!(
        err.starts_with("spend_ledger_unreadable:"),
        "expected error starting with spend_ledger_unreadable:, got: {err}"
    );
}
