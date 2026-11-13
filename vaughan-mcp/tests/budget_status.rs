//! Tests for the `budget_status` MCP tool.

use serde_json::json;
use tempfile::tempdir;
use vaughan_mcp::{McpContext, McpDispatcher};

static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn write_mode_0600(path: &std::path::Path, content: &str) {
    std::fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

fn handle_mock_status(store: &std::path::Path, _line: &str) -> String {
    let policy = match vaughan_agent::sentient::load_policy(store) {
        Ok(p) => p,
        Err(e) => {
            return serde_json::to_string(&json!({
                "ok": false,
                "code": "policy_load_failed",
                "message": format!("could not load policy: {e}"),
            }))
            .unwrap()
                + "\n";
        }
    };

    let view = match vaughan_agent::sentient::SpendLedger::snapshot(store) {
        Ok(v) => v,
        Err(e) => {
            return serde_json::to_string(&json!({
                "ok": false,
                "code": "spend_ledger_unreadable",
                "message": format!("spend_ledger_unreadable: {e}"),
            }))
            .unwrap()
                + "\n";
        }
    };

    let ledger_path = store.join("sentient-spend.json");
    let window_reset_unix = if view.window_start_unix == 0 {
        0
    } else {
        view.window_start_unix.saturating_add(view.window_seconds)
    };

    let mut assets_out = Vec::new();
    let mut asset_keys: Vec<_> = view.assets.keys().collect();
    asset_keys.sort();
    for key in asset_keys {
        let item = &view.assets[key];
        let cap = item
            .snapshot
            .saturating_mul(alloy::primitives::U256::from(policy.daily_cap_pct))
            .checked_div(alloy::primitives::U256::from(100))
            .unwrap_or(alloy::primitives::U256::ZERO);
        let remaining = cap.saturating_sub(item.spent);
        assets_out.push(json!({
            "asset": key,
            "snapshot": item.snapshot.to_string(),
            "spent": item.spent.to_string(),
            "cap": cap.to_string(),
            "remaining": remaining.to_string(),
        }));
    }

    let mut budget_json = json!({
        "profile": "sentient",
        "chain_id": 369,
        "enforcement": policy.enforcement.as_str(),
        "daily_cap_pct": policy.daily_cap_pct,
        "window_reset_unix": window_reset_unix,
        "assets": assets_out,
        "allow_transfer_to": policy.allow_transfer_to,
        "ledger_path": ledger_path.display().to_string(),
        "circuit_breaker_tripped": false,
    });
    if view.assets.is_empty() {
        budget_json["note"] = json!("no spend recorded this window");
    }

    serde_json::to_string(&json!({
        "ok": true,
        "address": "0x1111111111111111111111111111111111111111",
        "chain_id": 369,
        "store": store.display().to_string(),
        "policy_digest": policy.digest(),
        "budget": budget_json,
    }))
    .unwrap()
        + "\n"
}

async fn spawn_mock_signer(
    store: &std::path::Path,
) -> (std::path::PathBuf, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let socket_path = store.join("mock_signerd.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let store_buf = store.to_path_buf();

    let handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let store = store_buf.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut buf = BufReader::new(reader);
                let mut line = String::new();
                if buf.read_line(&mut line).await.is_ok() {
                    let resp = handle_mock_status(&store, &line);
                    let _ = writer.write_all(resp.as_bytes()).await;
                }
            });
        }
    });

    (socket_path, handle)
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
    let _guard = TEST_LOCK.lock().await;
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

    let (sock_path, server_task) = spawn_mock_signer(&sentient_dir).await;
    std::env::set_var("VAUGHAN_SIGNER_SOCKET", &sock_path);

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

    std::env::remove_var("VAUGHAN_SIGNER_SOCKET");
    server_task.abort();
}

#[tokio::test]
async fn test_budget_status_sentient_empty_ledger() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let sentient_dir = dir.path().join("sentient");
    std::fs::create_dir_all(&sentient_dir).unwrap();

    let policy_toml = r#"
enforcement = "enforced"
daily_cap_pct = 20
allow_transfer_to = []
"#;
    write_mode_0600(&sentient_dir.join("sentient-policy.toml"), policy_toml);

    let (sock_path, server_task) = spawn_mock_signer(&sentient_dir).await;
    std::env::set_var("VAUGHAN_SIGNER_SOCKET", &sock_path);

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

    std::env::remove_var("VAUGHAN_SIGNER_SOCKET");
    server_task.abort();
}

#[tokio::test]
async fn test_budget_status_corrupt_ledger_errors() {
    let _guard = TEST_LOCK.lock().await;
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

    let (sock_path, server_task) = spawn_mock_signer(&sentient_dir).await;
    std::env::set_var("VAUGHAN_SIGNER_SOCKET", &sock_path);

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

    std::env::remove_var("VAUGHAN_SIGNER_SOCKET");
    server_task.abort();
}
