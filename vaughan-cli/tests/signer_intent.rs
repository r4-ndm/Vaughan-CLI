//! Integration tests for `vaughan signerd` and `SignerIntent` execution.

use std::str::FromStr;
use std::sync::Arc;

use alloy::primitives::{Address, U256};
use secrecy::SecretString;
use vaughan_agent::sentient::policy::{AgentSessionPolicy, EnforcementMode};
use vaughan_cli::signerd::{
    is_peer_uid_allowed, MockBroadcaster, MockSwapQuoter, Signer, SignerResponse,
};
use vaughan_core::core::dex_routers::wpls_for_chain;
use vaughan_core::core::{
    decode_v3_swap_calldata, BatchTransferLeg, OperatingMode, SignerIntent, SwapFamily, WalletState,
};
use vaughan_core::security::hd_wallet::validate_mnemonic;

const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";
const TEST_CHAIN_ID: u64 = 943; // PulseChain testnet v4

fn setup_test_harness(
    allow_transfer_to: Vec<String>,
) -> (
    Signer,
    Arc<MockBroadcaster>,
    Arc<MockSwapQuoter>,
    tempfile::TempDir,
    Address,
) {
    let dir = tempfile::tempdir().unwrap();
    let wallet_path = dir.path().join("wallet.json");
    let mut wallet =
        WalletState::load_with_session(wallet_path, OperatingMode::SentientTrader, "sentient")
            .unwrap();
    let mnemonic = validate_mnemonic(TEST_MNEMONIC).unwrap();
    let password = SecretString::from("Password123!@#".to_string());
    wallet.create(&password, mnemonic).unwrap();
    wallet.unlock(&password).unwrap();
    wallet.set_active_network("pulsechain-testnet-v4").unwrap();

    let active_addr = wallet.active_address().unwrap().parse::<Address>().unwrap();

    let policy = AgentSessionPolicy {
        enforcement: EnforcementMode::Enforced,
        daily_cap_pct: 100,
        max_position_pct: 50,
        max_slippage_bps: 100, // 1% max slippage
        allow_transfer_to,
        ..Default::default()
    };
    vaughan_agent::sentient::save_policy(dir.path(), &policy).unwrap();

    let broadcaster = Arc::new(MockBroadcaster::new());
    let quoter = Arc::new(MockSwapQuoter::new());
    let rpc_url = "http://127.0.0.1:8545".to_string();

    let signer = Signer::new(
        dir.path().to_path_buf(),
        active_addr,
        TEST_CHAIN_ID,
        rpc_url,
        quoter.clone(),
        broadcaster.clone(),
    );

    (signer, broadcaster, quoter, dir, active_addr)
}

#[tokio::test]
async fn test_peer_uid_allowlist_function() {
    let allowlist = vec![1000, 1001];

    // Allowed UIDs
    assert!(is_peer_uid_allowed(1000, &allowlist));
    assert!(is_peer_uid_allowed(1001, &allowlist));

    // Refused UIDs
    assert!(!is_peer_uid_allowed(1002, &allowlist));
    assert!(!is_peer_uid_allowed(0, &allowlist));
    assert!(!is_peer_uid_allowed(65534, &allowlist));

    // Empty allowlist refuses all
    assert!(!is_peer_uid_allowed(1000, &[]));
}

#[tokio::test]
async fn test_status_op_returns_expected_fields() {
    let (signer, _, _, dir, active_addr) = setup_test_harness(vec![]);

    let req_json = r#"{"op":"status"}"#;
    let resp_str = signer.handle_line(req_json).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();

    assert!(resp.ok);
    assert_eq!(resp.address, Some(format!("{active_addr:#x}")));
    assert_eq!(resp.chain_id, Some(TEST_CHAIN_ID));
    assert_eq!(resp.store, Some(dir.path().display().to_string()));
    assert!(resp.policy_digest.is_some());
    assert_eq!(resp.policy_digest.unwrap().len(), 64);
}

#[tokio::test]
async fn test_transfer_to_address_outside_allowlist_refused() {
    let allowed_addr = Address::repeat_byte(0x11);
    let disallowed_addr = Address::repeat_byte(0x22);

    let (signer, _, _, _dir, _) = setup_test_harness(vec![format!("{allowed_addr:#x}")]);

    // Attempt transfer to disallowed address
    let intent = SignerIntent::Transfer {
        chain_id: TEST_CHAIN_ID,
        to: disallowed_addr,
        token: None,
        amount: U256::from(1000),
    };

    let req = serde_json::json!({
        "op": "sign",
        "intent": intent,
    });
    let resp_str = signer.handle_line(&req.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();

    assert!(!resp.ok);
    assert_eq!(resp.code, Some("destination_not_allowed".to_string()));
    assert!(resp.message.unwrap().contains("destination_not_allowed"));
}

#[tokio::test]
async fn test_swap_below_floor_refused_and_valid_swap_produces_exact_calldata() {
    let (signer, broadcaster, quoter, _dir, active_addr) = setup_test_harness(vec![]);

    // Wiz4rd V3 SwapRouter on chain 943
    let router = Address::from_str("0xfC656c95eCd418536844FeeaA46949bb9365BEaF").unwrap();
    let token_in = Address::repeat_byte(0x22);
    let token_out = Address::repeat_byte(0x33);
    let amount_in = U256::from(100_000);

    // Mock V3 quote returning 1,000,000. With 100 bps max slippage (1%), floor is 990,000.
    let quote_out = U256::from(1_000_000);
    let floor = U256::from(990_000);
    let pool_addr = Address::repeat_byte(0x88);
    quoter.set_v3(pool_addr, quote_out);

    // 1. Swap with min_amount_out below re-derived floor must be refused
    let intent_below = SignerIntent::Swap {
        chain_id: TEST_CHAIN_ID,
        family: SwapFamily::V3,
        router,
        path: vec![token_in, token_out],
        amount_in,
        min_amount_out: U256::from(980_000), // Below 990,000 floor
        fee: Some(3000),
    };
    let req_below = serde_json::json!({
        "op": "sign",
        "intent": intent_below,
    });
    let resp_str = signer.handle_line(&req_below.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("slippage_exceeded".to_string()));

    // 2. Valid swap with min_amount_out == floor must produce calldata with exact amountOutMinimum
    let intent_valid = SignerIntent::Swap {
        chain_id: TEST_CHAIN_ID,
        family: SwapFamily::V3,
        router,
        path: vec![token_in, token_out],
        amount_in,
        min_amount_out: floor,
        fee: Some(3000),
    };
    let req_valid = serde_json::json!({
        "op": "sign",
        "intent": intent_valid,
    });
    let resp_str = signer.handle_line(&req_valid.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(resp.ok);
    assert!(resp.tx_hash.is_some());

    // Verify wire calldata generated inside the signer
    let tx = broadcaster
        .get_last_tx()
        .expect("transaction was broadcast");
    let calldata_hex = tx.data.expect("V3 swap carries calldata");
    let calldata_bytes = hex::decode(calldata_hex.trim_start_matches("0x")).unwrap();
    let decoded = decode_v3_swap_calldata(&calldata_bytes).unwrap();

    assert_eq!(decoded.recipient, active_addr);
    assert_eq!(decoded.amount_in, amount_in);
    assert_eq!(decoded.amount_out_min, floor);
    assert_eq!(decoded.path, vec![token_in, token_out]);
}

#[tokio::test]
async fn test_replay_refused_and_reservation_released_on_send_failure() {
    let allowed_addr = Address::repeat_byte(0x11);
    let (signer, broadcaster, _, _dir, _) = setup_test_harness(vec![format!("{allowed_addr:#x}")]);

    let intent = SignerIntent::Transfer {
        chain_id: TEST_CHAIN_ID,
        to: allowed_addr,
        token: None,
        amount: U256::from(500),
    };
    let req = serde_json::json!({
        "op": "sign",
        "intent": intent,
    });

    // 1. Force broadcaster send failure
    broadcaster.set_fail(true);
    let resp_str = signer.handle_line(&req.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("broadcast_failed".to_string()));

    // 2. Retry of the same intent succeeds because reservation was released by release_legs
    broadcaster.set_fail(false);
    let resp_str = signer.handle_line(&req.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(resp.ok);
    assert!(resp.tx_hash.is_some());

    // 3. A second replay attempt of the exact same intent in the same window is refused
    let resp_str = signer.handle_line(&req.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("spend_already_reserved".to_string()));
    assert!(resp.message.unwrap().contains("spend_already_reserved"));
}

#[tokio::test]
async fn test_approve_with_u256_max_is_refused() {
    let router = Address::from_str("0x165c3410fc91ef562c50559f7d2289febed552d9").unwrap();
    let token = Address::repeat_byte(0x33);

    let (signer, _, _, _dir, _) = setup_test_harness(vec![]);

    let intent = SignerIntent::Approve {
        chain_id: TEST_CHAIN_ID,
        token,
        spender: router,
        amount: U256::MAX,
        expiry: None,
    };

    let req = serde_json::json!({
        "op": "sign",
        "intent": intent,
    });
    let resp_str = signer.handle_line(&req.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();

    assert!(!resp.ok);
    assert_eq!(resp.code, Some("approve_max_refused".to_string()));
    assert!(resp.message.unwrap().contains("U256::MAX"));
}

#[tokio::test]
async fn test_dca_slice_and_sweep_stealth_return_intent_not_implemented_naming_s2() {
    let (signer, _, _, _dir, _) = setup_test_harness(vec![]);

    // DCA slice
    let dca_intent = SignerIntent::DcaSlice {
        plan_id: "plan_1".into(),
        slice_index: 0,
        chain_id: TEST_CHAIN_ID,
        pair: Address::repeat_byte(0x44),
        amount_in: U256::from(100),
        corridor_min_out: U256::from(95),
    };
    let req_dca = serde_json::json!({
        "op": "sign",
        "intent": dca_intent,
    });
    let resp_str = signer.handle_line(&req_dca.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("intent_not_implemented".to_string()));
    assert!(resp.message.unwrap().contains("S2"));

    // Sweep stealth
    let sweep_intent = SignerIntent::SweepStealth {
        chain_id: TEST_CHAIN_ID,
        stealth_address: Address::repeat_byte(0x55),
    };
    let req_sweep = serde_json::json!({
        "op": "sign",
        "intent": sweep_intent,
    });
    let resp_str = signer.handle_line(&req_sweep.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("intent_not_implemented".to_string()));
    assert!(resp.message.unwrap().contains("S2"));
}

#[tokio::test]
async fn test_wrap_and_unwrap_intents_build_correct_calldata() {
    let (signer, broadcaster, _, _dir, _active_addr) = setup_test_harness(vec![]);
    let wpls = wpls_for_chain(TEST_CHAIN_ID).unwrap();

    // Wrap
    let wrap_intent = SignerIntent::Wrap {
        chain_id: TEST_CHAIN_ID,
        amount: U256::from(777),
    };
    let req_wrap = serde_json::json!({
        "op": "sign",
        "intent": wrap_intent,
    });
    let resp_str = signer.handle_line(&req_wrap.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(resp.ok);
    let tx = broadcaster.get_last_tx().unwrap();
    assert_eq!(tx.to, format!("{wpls:#x}"));
    assert_eq!(tx.value, "777");
    // WETH9 deposit selector: 0xd0e30db0
    assert_eq!(tx.data, Some("0xd0e30db0".to_string()));

    // Unwrap
    let unwrap_intent = SignerIntent::Unwrap {
        chain_id: TEST_CHAIN_ID,
        amount: U256::from(333),
    };
    let req_unwrap = serde_json::json!({
        "op": "sign",
        "intent": unwrap_intent,
    });
    let resp_str = signer.handle_line(&req_unwrap.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(resp.ok);
    let tx = broadcaster.get_last_tx().unwrap();
    assert_eq!(tx.to, format!("{wpls:#x}"));
    assert_eq!(tx.value, "0");
    // WETH9 withdraw selector: 0x2e1a7d4d
    assert!(tx.data.unwrap().starts_with("0x2e1a7d4d"));
}

#[tokio::test]
async fn test_batch_transfer_builds_7702_execute() {
    let dest1 = Address::repeat_byte(0x11);
    let dest2 = Address::repeat_byte(0x22);

    let (signer, broadcaster, _, _dir, active_addr) =
        setup_test_harness(vec![format!("{dest1:#x}"), format!("{dest2:#x}")]);

    let batch_intent = SignerIntent::BatchTransfer {
        chain_id: TEST_CHAIN_ID,
        legs: vec![
            BatchTransferLeg {
                to: dest1,
                amount: U256::from(100),
            },
            BatchTransferLeg {
                to: dest2,
                amount: U256::from(200),
            },
        ],
    };

    let req = serde_json::json!({
        "op": "sign",
        "intent": batch_intent,
    });
    let resp_str = signer.handle_line(&req.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(resp.ok);

    let tx = broadcaster.get_last_tx().unwrap();
    assert_eq!(tx.to, format!("{active_addr:#x}"));
    assert_eq!(tx.value, "300");

    let calldata_bytes = hex::decode(tx.data.unwrap().trim_start_matches("0x")).unwrap();
    let txns = vaughan_aa::decode_execute(&calldata_bytes).unwrap();
    assert_eq!(txns.len(), 2);
    assert_eq!(txns[0].to, dest1);
    assert_eq!(txns[0].value, U256::from(100));
    assert_eq!(txns[1].to, dest2);
    assert_eq!(txns[1].value, U256::from(200));
}

#[tokio::test]
async fn test_status_op_returns_budget_and_breaker_info() {
    let (signer, _, _, dir, _) = setup_test_harness(vec![]);

    let req_json = r#"{"op":"status"}"#;
    let resp_str = signer.handle_line(req_json).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();

    assert!(resp.ok);
    assert!(resp.budget.is_some());
    let budget = resp.budget.unwrap();
    assert_eq!(budget["profile"], "sentient");
    assert_eq!(budget["enforcement"], "enforced");
    assert_eq!(budget["daily_cap_pct"], 100);
    assert_eq!(budget["circuit_breaker_tripped"], false);
    assert_eq!(
        budget["ledger_path"],
        dir.path().join("sentient-spend.json").display().to_string()
    );
}

#[tokio::test]
async fn test_circuit_breaker_tripped_refuses_sign_and_reports_status() {
    let allowed_addr = Address::repeat_byte(0x11);
    let (signer, _, _, _, _) = setup_test_harness(vec![format!("{allowed_addr:#x}")]);

    // Manually trip the circuit breaker inside the signer
    signer.breaker().trip("emergency stop triggered in test");

    // 1. Status reports breaker tripped
    let status_resp_str = signer.handle_line(r#"{"op":"status"}"#).await;
    let status_resp: SignerResponse = serde_json::from_str(&status_resp_str).unwrap();
    assert!(status_resp.ok);
    let budget = status_resp.budget.unwrap();
    assert_eq!(budget["circuit_breaker_tripped"], true);
    assert!(budget["circuit_breaker_reason"]
        .as_str()
        .unwrap()
        .contains("emergency stop"));

    // 2. Sign intent is refused by breaker
    let intent = SignerIntent::Transfer {
        chain_id: TEST_CHAIN_ID,
        to: allowed_addr,
        token: None,
        amount: U256::from(100),
    };
    let req = serde_json::json!({
        "op": "sign",
        "intent": intent,
    });
    let sign_resp_str = signer.handle_line(&req.to_string()).await;
    let sign_resp: SignerResponse = serde_json::from_str(&sign_resp_str).unwrap();
    assert!(!sign_resp.ok);
    assert_eq!(sign_resp.code, Some("circuit_breaker_tripped".to_string()));
    assert!(sign_resp
        .message
        .unwrap()
        .contains("emergency stop triggered in test"));
}

#[tokio::test]
async fn test_signer_client_over_real_unix_socket() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use vaughan_cli::signerd::SignerClient;

    let dest = Address::repeat_byte(0x55);
    let (signer, _, _, dir, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    let socket_path = dir.path().join("test_signerd.sock");

    let listener = UnixListener::bind(&socket_path).unwrap();
    let signer_c = signer.clone();

    // Spawn real socket listener loop
    let server_task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let signer = signer_c.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut buf = BufReader::new(reader);
                let mut line = String::new();
                if buf.read_line(&mut line).await.is_ok() {
                    let resp = signer.handle_line(&line).await + "\n";
                    let _ = writer.write_all(resp.as_bytes()).await;
                }
            });
        }
    });

    let client = SignerClient::new(&socket_path);

    // 1. Ping
    assert!(client.ping().await);

    // 2. Status
    let status = client.status().await.unwrap();
    assert!(status.ok);
    assert!(status.budget.is_some());

    // 3. Valid sign intent
    let valid_intent = SignerIntent::Transfer {
        chain_id: TEST_CHAIN_ID,
        to: dest,
        token: None,
        amount: U256::from(500),
    };
    let tx_hash = client.execute(&valid_intent).await.unwrap();
    assert!(!tx_hash.is_empty());

    // 4. Refused sign intent (disallowed destination)
    let disallowed_dest = Address::repeat_byte(0x99);
    let invalid_intent = SignerIntent::Transfer {
        chain_id: TEST_CHAIN_ID,
        to: disallowed_dest,
        token: None,
        amount: U256::from(500),
    };
    let err = client.execute(&invalid_intent).await.unwrap_err();
    assert!(
        err.starts_with("destination_not_allowed"),
        "expected destination_not_allowed prefix, got: {err}"
    );

    server_task.abort();
}

#[tokio::test]
async fn test_socket_peer_uid_rejection() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixListener;
    use vaughan_cli::signerd::SignerClient;

    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("peer_reject.sock");

    let listener = UnixListener::bind(&socket_path).unwrap();
    // Allowlist that does NOT contain our current UID
    let current_uid = rustix::process::getuid().as_raw();
    let allowlist = vec![current_uid + 100];

    let server_task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let allowed = allowlist.clone();
            tokio::spawn(async move {
                let cred = stream.peer_cred().unwrap();
                let peer_uid = cred.uid();
                if !is_peer_uid_allowed(peer_uid, &allowed) {
                    let resp = serde_json::to_string(&SignerResponse::error(
                        "unauthorized",
                        "peer UID not permitted",
                    ))
                    .unwrap()
                        + "\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
        }
    });

    let client = SignerClient::new(&socket_path);
    let err = client.status().await.unwrap_err();
    assert!(
        err.starts_with("unauthorized"),
        "expected unauthorized error, got: {err}"
    );

    server_task.abort();
}
