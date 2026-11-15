//! Integration tests for `vaughan signerd` and `SignerIntent` execution.

use std::str::FromStr;
use std::sync::Arc;

use alloy::primitives::{Address, U256};
use secrecy::SecretString;
use vaughan_agent::sentient::policy::{AgentSessionPolicy, EnforcementMode};
use vaughan_cli::signerd::{
    is_peer_uid_allowed, MockBroadcaster, MockSwapQuoter, Signer, SignerResponse,
    STEP_UP_UNSUPPORTED_CODE, STEP_UP_UNSUPPORTED_MSG,
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
async fn test_dca_slice_refusals_and_execution() {
    let (signer, broadcaster, quoter, dir, active_addr) = setup_test_harness(vec![]);
    let pair = Address::repeat_byte(0x44);
    let amount_in = U256::from(100_000);

    // 1. Refusal on zero corridor_min_out
    let intent_zero_corridor = SignerIntent::DcaSlice {
        plan_id: "plan_1".into(),
        slice_index: 0,
        chain_id: TEST_CHAIN_ID,
        pair,
        amount_in,
        corridor_min_out: U256::ZERO,
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent_zero_corridor });
    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("zero_corridor_min_out".to_string()));

    // 2. Refusal on unknown swap kind (e.g. mock quoter returns unknown)
    quoter.set_force_kind(vaughan_core::core::proposal::DexSwapKind::Unknown);
    let intent_unknown = SignerIntent::DcaSlice {
        plan_id: "plan_1".into(),
        slice_index: 0,
        chain_id: TEST_CHAIN_ID,
        pair,
        amount_in,
        corridor_min_out: U256::from(50_000),
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent_unknown });
    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("unknown_swap_kind".to_string()));

    // 3. Refusal on zero quote
    quoter.set_v3(pair, U256::ZERO);
    quoter.set_force_kind(vaughan_core::core::proposal::DexSwapKind::V3 {
        pool: pair,
        fee: 3000,
        quote: U256::ZERO,
    });
    let intent_zero_quote = SignerIntent::DcaSlice {
        plan_id: "plan_1".into(),
        slice_index: 0,
        chain_id: TEST_CHAIN_ID,
        pair,
        amount_in,
        corridor_min_out: U256::from(50_000),
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent_zero_quote });
    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("zero_quote".to_string()));

    // 4. Refusal when quote is below corridor (naming both numbers)
    let fresh_quote = U256::from(80_000);
    let corridor_min = U256::from(95_000);
    quoter.set_v3(pair, fresh_quote);
    quoter.set_force_kind(vaughan_core::core::proposal::DexSwapKind::V3 {
        pool: pair,
        fee: 3000,
        quote: fresh_quote,
    });
    let intent_below = SignerIntent::DcaSlice {
        plan_id: "plan_1".into(),
        slice_index: 0,
        chain_id: TEST_CHAIN_ID,
        pair,
        amount_in,
        corridor_min_out: corridor_min,
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent_below });
    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("quote_below_corridor".to_string()));
    let msg = resp.message.unwrap();
    assert!(
        msg.contains("80000") && msg.contains("95000"),
        "expected both numbers in error: {msg}"
    );

    // 5. Valid DCA slice: builds calldata with exact amountOutMinimum matching corridor value
    let valid_quote = U256::from(100_000);
    let valid_corridor = U256::from(95_000);
    let token_in = Address::repeat_byte(0x22);
    let token_out = Address::repeat_byte(0x33);
    quoter.set_v3(pair, valid_quote);
    quoter.set_dca_tokens(token_in, token_out);
    let intent_valid = SignerIntent::DcaSlice {
        plan_id: "plan_valid".into(),
        slice_index: 0,
        chain_id: TEST_CHAIN_ID,
        pair,
        amount_in,
        corridor_min_out: valid_corridor,
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent_valid });
    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(resp.ok);
    assert!(resp.tx_hash.is_some());

    let tx = broadcaster.get_last_tx().expect("tx broadcast");
    let calldata_bytes = hex::decode(tx.data.unwrap().trim_start_matches("0x")).unwrap();
    let decoded = decode_v3_swap_calldata(&calldata_bytes).unwrap();
    assert_eq!(decoded.recipient, active_addr);
    assert_eq!(decoded.amount_in, amount_in);
    assert_eq!(decoded.amount_out_min, valid_corridor);

    // 6. Replay prevention: submitting same intent again is refused with spend_already_reserved
    let resp_replay: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(!resp_replay.ok);
    assert_eq!(resp_replay.code, Some("spend_already_reserved".to_string()));

    // 7. Failure release: failing broadcast releases ledger reservation
    broadcaster.set_fail(true);
    let intent_fail = SignerIntent::DcaSlice {
        plan_id: "plan_fail".into(),
        slice_index: 1,
        chain_id: TEST_CHAIN_ID,
        pair,
        amount_in,
        corridor_min_out: valid_corridor,
    };
    let req_fail = serde_json::json!({ "op": "sign", "intent": intent_fail });
    let resp_fail: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req_fail.to_string()).await).unwrap();
    assert!(!resp_fail.ok);
    assert_eq!(resp_fail.code, Some("broadcast_failed".to_string()));

    // Check that reservation was released from recent
    let ledger = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    assert!(!ledger
        .recent
        .iter()
        .any(|r| r.key == intent_fail.idempotency_key()));

    // Retry reaches broadcast again
    broadcaster.set_fail(false);
    let resp_retry: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req_fail.to_string()).await).unwrap();
    assert!(resp_retry.ok);

    // 8. Restart durability: drop signer and rebuild from same store; spend/recent unchanged
    let snap_before = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    drop(signer);
    let signer2 = Signer::new(
        dir.path().to_path_buf(),
        active_addr,
        TEST_CHAIN_ID,
        "http://127.0.0.1:8545".to_string(),
        quoter.clone(),
        broadcaster.clone(),
    );
    let snap_after = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    assert_eq!(snap_before.assets, snap_after.assets);
    assert_eq!(snap_before.recent, snap_after.recent);

    // Replay on rebuilt signer is still refused with spend_already_reserved
    let resp_rebuilt_replay: SignerResponse =
        serde_json::from_str(&signer2.handle_line(&req_fail.to_string()).await).unwrap();
    assert!(!resp_rebuilt_replay.ok);
    assert_eq!(
        resp_rebuilt_replay.code,
        Some("spend_already_reserved".to_string())
    );
}

#[tokio::test]
async fn test_sweep_stealth_refusals_and_execution() {
    let (signer, broadcaster, quoter, dir, active_addr) = setup_test_harness(vec![]);
    let stealth_addr = Address::repeat_byte(0x55);

    // 1. Refusal on unknown stealth note (not in stealth-pending.json)
    let intent = SignerIntent::SweepStealth {
        chain_id: TEST_CHAIN_ID,
        stealth_address: stealth_addr,
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent });
    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code, Some("unknown_stealth_note".to_string()));

    // Record stealth address in stealth-pending.json
    vaughan_agent::sentient::record_stealth_pending(dir.path(), stealth_addr).unwrap();

    // 2. Refusal when amount cannot be determined
    signer.set_balance(Some(stealth_addr), U256::ZERO);
    let resp_no_amt: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(!resp_no_amt.ok);
    assert_eq!(resp_no_amt.code, Some("amount_undetermined".to_string()));

    // 3. Valid sweep with known amount
    let sweep_amount = U256::from(500_000_000);
    signer.set_balance(Some(stealth_addr), sweep_amount);
    let resp_ok: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(resp_ok.ok);
    assert!(resp_ok.tx_hash.is_some());

    // 4. Replay prevention: submitting same sweep intent again is refused with spend_already_reserved
    let resp_replay: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(!resp_replay.ok);
    assert_eq!(resp_replay.code, Some("spend_already_reserved".to_string()));

    // 5. Failure release: failing broadcast releases ledger reservation
    let stealth_addr2 = Address::repeat_byte(0x66);
    vaughan_agent::sentient::record_stealth_pending(dir.path(), stealth_addr2).unwrap();
    signer.set_balance(Some(stealth_addr2), sweep_amount);

    broadcaster.set_fail(true);
    let intent2 = SignerIntent::SweepStealth {
        chain_id: TEST_CHAIN_ID,
        stealth_address: stealth_addr2,
    };
    let req2 = serde_json::json!({ "op": "sign", "intent": intent2 });
    let resp_fail: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req2.to_string()).await).unwrap();
    assert!(!resp_fail.ok);
    assert_eq!(resp_fail.code, Some("broadcast_failed".to_string()));

    let ledger = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    assert!(!ledger
        .recent
        .iter()
        .any(|r| r.key == intent2.idempotency_key()));

    // Retry succeeds once failure is cleared
    broadcaster.set_fail(false);
    let resp_retry: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req2.to_string()).await).unwrap();
    assert!(resp_retry.ok);

    // 6. Restart durability: drop signer and rebuild from same store; spend/recent unchanged
    let snap_before = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    drop(signer);
    let signer2 = Signer::new(
        dir.path().to_path_buf(),
        active_addr,
        TEST_CHAIN_ID,
        "http://127.0.0.1:8545".to_string(),
        quoter.clone(),
        broadcaster.clone(),
    );
    let snap_after = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    assert_eq!(snap_before.assets, snap_after.assets);
    assert_eq!(snap_before.recent, snap_after.recent);

    // Replay on rebuilt signer is still refused with spend_already_reserved
    signer2.set_balance(Some(stealth_addr2), sweep_amount);
    let resp_rebuilt_replay: SignerResponse =
        serde_json::from_str(&signer2.handle_line(&req2.to_string()).await).unwrap();
    assert!(!resp_rebuilt_replay.ok);
    assert_eq!(
        resp_rebuilt_replay.code,
        Some("spend_already_reserved".to_string())
    );
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

#[tokio::test]
async fn test_native_in_swap_and_dca_sets_tx_value_and_unknown_chain_refuses() {
    let (signer, broadcaster, quoter, _dir, active_addr) = setup_test_harness(vec![]);

    let wpls_943 = wpls_for_chain(TEST_CHAIN_ID).unwrap();
    let token_out = Address::repeat_byte(0x33);
    let router_v3 = Address::from_str("0xfC656c95eCd418536844FeeaA46949bb9365BEaF").unwrap();
    let router_v2 = Address::from_str("0xDaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7").unwrap();
    let amount_in = U256::from(500_000_000_000_000_000u64); // 0.5 tPLS
    let floor = U256::from(990_000);

    let pool_addr = Address::repeat_byte(0x88);
    quoter.set_v3(pool_addr, U256::from(1_000_000));
    quoter.set_v2(U256::from(1_000_000));

    // 1. Native-in V3 swap must produce tx.value == amount_in
    let intent_v3_native = SignerIntent::Swap {
        chain_id: TEST_CHAIN_ID,
        family: SwapFamily::V3,
        router: router_v3,
        path: vec![wpls_943, token_out],
        amount_in,
        min_amount_out: floor,
        fee: Some(3000),
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent_v3_native });
    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(resp.ok);
    let tx = broadcaster.get_last_tx().expect("tx broadcast");
    assert_eq!(tx.value, amount_in.to_string());

    // 2. Native-in V2 swap must produce tx.value == amount_in
    let intent_v2_native = SignerIntent::Swap {
        chain_id: TEST_CHAIN_ID,
        family: SwapFamily::V2,
        router: router_v2,
        path: vec![wpls_943, token_out],
        amount_in,
        min_amount_out: floor,
        fee: None,
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent_v2_native });
    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(resp.ok);
    let tx = broadcaster.get_last_tx().expect("tx broadcast");
    assert_eq!(tx.value, amount_in.to_string());

    // 3. Native-in DCA slice must produce tx.value == amount_in
    quoter.set_v3(pool_addr, U256::from(100_000));
    quoter.set_dca_tokens(wpls_943, token_out);
    let intent_dca_native = SignerIntent::DcaSlice {
        plan_id: "plan_val_dca".into(),
        slice_index: 0,
        chain_id: TEST_CHAIN_ID,
        pair: pool_addr,
        amount_in,
        corridor_min_out: U256::from(95_000),
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent_dca_native });
    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap();
    assert!(resp.ok);
    let tx = broadcaster.get_last_tx().expect("tx broadcast");
    assert_eq!(tx.value, amount_in.to_string());

    // 4. Refusal when wrapped native cannot be resolved for chain
    assert!(vaughan_cli::signerd::intents::build_v3_swap_tx(
        999_999,
        active_addr,
        router_v3,
        wpls_943,
        token_out,
        3000,
        amount_in,
        floor,
        active_addr,
    )
    .is_err());

    assert!(vaughan_cli::signerd::intents::build_v2_swap_tx(
        999_999,
        active_addr,
        router_v2,
        vec![wpls_943, token_out],
        amount_in,
        floor,
        active_addr,
    )
    .is_err());
}

// ---------------------------------------------------------------------------
// S3 / S3c: policy & root-approved policy_update integration tests
// ---------------------------------------------------------------------------

const S3_TEST_PW: &str = "S3PolicyStepUp!99";
const S3_TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";

fn setup_s3_store(
    allow_transfer_to: Vec<String>,
    daily_cap_pct: u8,
    max_position_pct: u8,
) -> (
    Signer,
    Arc<MockBroadcaster>,
    tempfile::TempDir,
    Address,
    std::path::PathBuf,
) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    let wallet_path = dir.path().join("wallet.json");
    let mut wallet =
        WalletState::load_with_session(wallet_path, OperatingMode::SentientTrader, "sentient")
            .unwrap();
    let pw = secrecy::SecretString::from(S3_TEST_PW.to_string());
    wallet.restore(&pw, S3_TEST_MNEMONIC).unwrap();
    let active_addr: Address = wallet.active_address().unwrap().parse().unwrap();

    let policy = vaughan_agent::sentient::AgentSessionPolicy {
        allow_transfer_to,
        daily_cap_pct,
        max_position_pct,
        required_rpc_quorum: 1,
        ..vaughan_agent::sentient::AgentSessionPolicy::default()
    };
    vaughan_agent::sentient::save_policy(dir.path(), &policy).unwrap();

    let policy_path = dir
        .path()
        .join(vaughan_agent::sentient::SENTIENT_POLICY_TOML);
    let broadcaster = Arc::new(MockBroadcaster::new());
    let quoter = Arc::new(MockSwapQuoter::new());
    let signer = Signer::new(
        dir.path().to_path_buf(),
        active_addr,
        TEST_CHAIN_ID,
        "http://127.0.0.1:8545".to_string(),
        quoter,
        broadcaster.clone(),
    );
    (signer, broadcaster, dir, active_addr, policy_path)
}

#[cfg(unix)]
async fn spawn_s3_socket_server(
    signer: Signer,
    allow_uid: Vec<u32>,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let sock_dir = tempfile::tempdir().unwrap();
    let sock_path = sock_dir.path().join("signerd-s3.sock");
    let listener = UnixListener::bind(&sock_path).unwrap();

    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let signer = signer.clone();
            let allow = allow_uid.clone();
            tokio::spawn(async move {
                let peer_uid = match stream.peer_cred() {
                    Ok(cred) => cred.uid(),
                    Err(_) => return,
                };
                if !is_peer_uid_allowed(peer_uid, &allow) {
                    let resp = serde_json::to_string(&SignerResponse::error(
                        "unauthorized",
                        "peer UID not permitted",
                    ))
                    .unwrap()
                        + "\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                    return;
                }
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    let resp = signer.handle_line_with_uid(line.trim(), peer_uid).await;
                    let _ = stream.write_all(format!("{resp}\n").as_bytes()).await;
                }
            });
        }
    });

    (sock_dir, sock_path, task)
}

#[cfg(unix)]
async fn send_raw_socket_json(sock_path: &std::path::Path, raw_json: &str) -> SignerResponse {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let mut stream = UnixStream::connect(sock_path).await.unwrap();
    stream
        .write_all(format!("{raw_json}\n").as_bytes())
        .await
        .unwrap();
    stream.flush().await.unwrap();
    let mut reader = BufReader::new(stream);
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).await.unwrap();
    serde_json::from_str(resp_line.trim()).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3c4_policy_update_without_auth_or_with_password_refused_and_password_key_rejected() {
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 5, 50);
    signer.set_expected_approval_uid(current_uid);
    let baseline = std::fs::read(&policy_path).unwrap();
    let (_sock_dir, sock_path, srv) =
        spawn_s3_socket_server(signer.clone(), vec![current_uid]).await;

    let client = vaughan_core::core::SignerClient::new(&sock_path);
    let current = client.policy().await.unwrap();
    let cur_digest = current.policy_digest.clone();
    let mut modified = current.policy.unwrap();
    modified["daily_cap_pct"] = serde_json::json!(90);

    // Measure Argon2id vault unlock latency on this store for relative no-KDF check.
    let kdf_start = std::time::Instant::now();
    let mut kdf_wallet = WalletState::load_with_session(
        dir.path().join("wallet.json"),
        OperatingMode::SentientTrader,
        "sentient",
    )
    .unwrap();
    kdf_wallet
        .unlock(&secrecy::SecretString::from(S3_TEST_PW.to_string()))
        .unwrap();
    let kdf_elapsed = kdf_start.elapsed();

    let audit_path = dir.path().join(vaughan_agent::sentient::POLICY_AUDIT_JSONL);

    // 1. auth: None with no password key -> step_up_unsupported, store unchanged, no audit file, fast relative to KDF.
    let no_pw_req = serde_json::json!({
        "op": "policy_update",
        "policy": modified,
        "expected_digest": cur_digest,
    });
    let no_pw_start = std::time::Instant::now();
    let r_no_pw = send_raw_socket_json(&sock_path, &no_pw_req.to_string()).await;
    let no_pw_elapsed = no_pw_start.elapsed();
    assert!(!r_no_pw.ok);
    assert_eq!(r_no_pw.code.as_deref(), Some(STEP_UP_UNSUPPORTED_CODE));
    assert_eq!(r_no_pw.message.as_deref(), Some(STEP_UP_UNSUPPORTED_MSG));
    assert!(
        no_pw_elapsed < kdf_elapsed / 4,
        "auth:None refusal must skip KDF and return fast (reply={no_pw_elapsed:?}, kdf={kdf_elapsed:?})"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);
    assert!(!audit_path.exists());

    // 2. Legacy password key on auth: None (wrong or right password) -> rejected as unknown field (invalid_json), no KDF.
    let wrong_pw_req = serde_json::json!({
        "op": "policy_update",
        "password": "WrongPassword!999",
        "policy": modified,
        "expected_digest": cur_digest,
    });
    let r_wrong = send_raw_socket_json(&sock_path, &wrong_pw_req.to_string()).await;
    assert!(!r_wrong.ok);
    assert_eq!(r_wrong.code.as_deref(), Some("invalid_json"));
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);
    assert!(!audit_path.exists());

    let right_pw_req = serde_json::json!({
        "op": "policy_update",
        "password": S3_TEST_PW,
        "policy": modified,
        "expected_digest": cur_digest,
    });
    let right_pw_start = std::time::Instant::now();
    let r_right = send_raw_socket_json(&sock_path, &right_pw_req.to_string()).await;
    let right_pw_elapsed = right_pw_start.elapsed();
    assert!(!r_right.ok);
    assert_eq!(r_right.code, r_wrong.code);
    assert_eq!(r_right.message, r_wrong.message);
    assert!(
        right_pw_elapsed < kdf_elapsed / 4,
        "password-bearing request must skip KDF (reply={right_pw_elapsed:?}, kdf={kdf_elapsed:?})"
    );
    eprintln!(
        "s3c4 no-KDF timing: vault_unlock={kdf_elapsed:?}, no_auth_reply={no_pw_elapsed:?}, right_pw_rejected={right_pw_elapsed:?}"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);
    assert!(!audit_path.exists());

    // 3. auth: None and password-bearing requests do NOT consume the per-uid step_up_limiter.
    let limiter_uid = 3001u32;
    for _ in 0..10 {
        let resp_no_pw: SignerResponse = serde_json::from_str(
            &signer
                .handle_line_with_uid(&no_pw_req.to_string(), limiter_uid)
                .await,
        )
        .unwrap();
        assert_eq!(resp_no_pw.code.as_deref(), Some(STEP_UP_UNSUPPORTED_CODE));

        let resp_pw: SignerResponse = serde_json::from_str(
            &signer
                .handle_line_with_uid(&right_pw_req.to_string(), limiter_uid)
                .await,
        )
        .unwrap();
        assert_eq!(resp_pw.code.as_deref(), Some("invalid_json"));
    }
    let approved_probe = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": modified,
    });
    let after_probe: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_probe.to_string(), limiter_uid)
            .await,
    )
    .unwrap();
    assert_eq!(
        after_probe.code.as_deref(),
        Some("step_up_failed"),
        "auth: None and invalid_json refusals must not consume step_up_limiter budget"
    );

    // 4. password key on auth: "stage" or auth: "approved" is rejected with invalid_json and leaves pending/approval untouched.
    let pending_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME);
    let stage_with_pw = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "password": S3_TEST_PW,
        "policy": modified,
        "expected_digest": cur_digest,
    });
    let r_stage = send_raw_socket_json(&sock_path, &stage_with_pw.to_string()).await;
    assert!(!r_stage.ok);
    assert_eq!(r_stage.code.as_deref(), Some("invalid_json"));
    assert!(!pending_path.exists());
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);

    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 90;
    let appr = write_s3b_approval(dir.path(), &p0, &p1, 120);
    let approved_with_pw = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "password": S3_TEST_PW,
        "policy": p1,
    });
    let r_appr_pw = send_raw_socket_json(&sock_path, &approved_with_pw.to_string()).await;
    assert!(!r_appr_pw.ok);
    assert_eq!(r_appr_pw.code.as_deref(), Some("invalid_json"));
    assert!(
        appr.exists(),
        "rejected password request must not consume approval"
    );
    assert!(
        pending_path.exists(),
        "rejected password request must not clear pending policy"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);
    assert!(!audit_path.exists());
    std::fs::remove_file(&appr).unwrap();
    std::fs::remove_file(&pending_path).unwrap();

    srv.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn test_policy_stage_refusals_write_no_pending_and_two_step_disable() {
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 10, 10);
    let uid = rustix::process::geteuid().as_raw();
    signer.set_expected_approval_uid(uid);
    let (_sock_dir, sock_path, srv) = spawn_s3_socket_server(signer, vec![uid]).await;
    let client = vaughan_core::core::SignerClient::new(&sock_path);

    let baseline_bytes = std::fs::read(&policy_path).unwrap();
    let current = client.policy().await.unwrap();
    let cur_digest = current.policy_digest.unwrap();
    let base_val = current.policy.unwrap();

    // Helper closure assertion via raw socket so we can inspect exact refusal codes & invalid JSON shapes.
    let cases: Vec<(&str, serde_json::Value, &str)> = vec![
        (
            "daily_cap_pct: 101",
            {
                let mut p = base_val.clone();
                p["daily_cap_pct"] = serde_json::json!(101);
                serde_json::json!({
                    "op": "policy_update",
                    "auth": "stage",
                    "policy": p,
                    "expected_digest": cur_digest,
                })
            },
            "policy_invalid",
        ),
        (
            "max_slippage_bps: 10000",
            {
                let mut p = base_val.clone();
                p["max_slippage_bps"] = serde_json::json!(10_000);
                serde_json::json!({
                    "op": "policy_update",
                    "auth": "stage",
                    "policy": p,
                    "expected_digest": cur_digest,
                })
            },
            "policy_invalid",
        ),
        (
            "zero address in allow_transfer_to",
            {
                let mut p = base_val.clone();
                p["allow_transfer_to"] =
                    serde_json::json!(["0x0000000000000000000000000000000000000000"]);
                serde_json::json!({
                    "op": "policy_update",
                    "auth": "stage",
                    "policy": p,
                    "expected_digest": cur_digest,
                })
            },
            "policy_invalid",
        ),
        (
            "enforcement: disabled without prior acknowledge_unsafe",
            {
                let mut p = base_val.clone();
                p["enforcement"] = serde_json::json!("disabled");
                p["acknowledge_unsafe"] = serde_json::json!(true);
                serde_json::json!({
                    "op": "policy_update",
                    "auth": "stage",
                    "policy": p,
                    "expected_digest": cur_digest,
                })
            },
            "policy_refused",
        ),
        (
            "expected_digest mismatch",
            serde_json::json!({
                "op": "policy_update",
                "auth": "stage",
                "policy": base_val,
                "expected_digest": "0000000000000000000000000000000000000000000000000000000000000000",
            }),
            "policy_conflict",
        ),
        (
            "unknown top-level request field",
            serde_json::json!({
                "op": "policy_update",
                "auth": "stage",
                "policy": base_val,
                "evil_extra_request_field": true,
            }),
            "invalid_json",
        ),
        (
            "unknown policy field",
            {
                let mut p = base_val.clone();
                p["unknown_policy_key"] = serde_json::json!(42);
                serde_json::json!({
                    "op": "policy_update",
                    "auth": "stage",
                    "policy": p,
                    "expected_digest": cur_digest,
                })
            },
            "policy_invalid",
        ),
    ];

    for (label, req_json, expected_code) in cases {
        let resp = send_raw_socket_json(&sock_path, &req_json.to_string()).await;
        assert!(!resp.ok, "{label}: expected refusal, got ok");
        assert_eq!(
            resp.code.as_deref(),
            Some(expected_code),
            "{label}: unexpected code: {resp:?}"
        );
        assert_eq!(
            std::fs::read(&policy_path).unwrap(),
            baseline_bytes,
            "{label}: store must remain unchanged on refusal"
        );
        assert!(
            !dir.path()
                .join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME)
                .exists(),
            "{label}: refused stage must not write policy-pending.json"
        );
    }

    // Two-step disable works once acknowledge_unsafe=true is already persisted on disk.
    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut step1_pol = p0.clone();
    step1_pol.acknowledge_unsafe = true;
    let step1_val = serde_json::to_value(&step1_pol).unwrap();
    let s1 = client
        .policy_stage(step1_val.clone(), Some(cur_digest))
        .await
        .unwrap();
    assert_eq!(s1.code.as_deref(), Some("step_up_required"));
    write_s3b_approval(dir.path(), &p0, &step1_pol, 120);
    let r1 = client
        .policy_approved(step1_val, Some(p0.digest()))
        .await
        .unwrap();
    assert!(r1.ok, "step1 approval failed: {r1:?}");

    let mut step2_pol = step1_pol.clone();
    step2_pol.enforcement = EnforcementMode::Disabled;
    let step2_val = serde_json::to_value(&step2_pol).unwrap();
    let s2 = client
        .policy_stage(step2_val.clone(), r1.policy_digest.clone())
        .await
        .unwrap();
    assert_eq!(s2.code.as_deref(), Some("step_up_required"));
    write_s3b_approval(dir.path(), &step1_pol, &step2_pol, 120);
    let r2 = client
        .policy_approved(step2_val, r1.policy_digest)
        .await
        .unwrap();
    assert!(r2.ok, "step2 approval failed: {r2:?}");

    srv.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn test_policy_update_audit_rotates_at_200_lines_over_socket() {
    use std::os::unix::fs::PermissionsExt;

    let (signer, _, dir, _, _) = setup_s3_store(vec![], 10, 10);
    let uid = rustix::process::geteuid().as_raw();
    signer.set_expected_approval_uid(uid);

    let audit_path = dir.path().join(vaughan_agent::sentient::POLICY_AUDIT_JSONL);
    let mut seeded = String::new();
    for i in 0..200 {
        seeded.push_str(&format!(
            "{{\"ts\":{i},\"field_changes\":{{}},\"source\":\"socket\"}}\n"
        ));
    }
    std::fs::write(&audit_path, seeded).unwrap();
    std::fs::set_permissions(&audit_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let (_sock_dir, sock_path, srv) = spawn_s3_socket_server(signer, vec![uid]).await;
    let client = vaughan_core::core::SignerClient::new(&sock_path);
    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 42;
    let p1_val = serde_json::to_value(&p1).unwrap();

    let stage_resp = client
        .policy_stage(p1_val.clone(), Some(p0.digest()))
        .await
        .unwrap();
    assert_eq!(stage_resp.code.as_deref(), Some("step_up_required"));
    write_s3b_approval(dir.path(), &p0, &p1, 120);

    let resp = client
        .policy_approved(p1_val, Some(p0.digest()))
        .await
        .unwrap();
    assert!(resp.ok, "expected approved audit update ok, got {resp:?}");

    let after = std::fs::read_to_string(&audit_path).unwrap();
    let lines: Vec<&str> = after.lines().collect();
    assert_eq!(lines.len(), 200);
    let oldest: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(oldest["ts"], 1);
    let newest: serde_json::Value = serde_json::from_str(lines[199]).unwrap();
    assert_eq!(newest["field_changes"]["daily_cap_pct"]["from"], 10);
    assert_eq!(newest["field_changes"]["daily_cap_pct"]["to"], 42);

    srv.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn test_policy_update_peer_uid_outside_allowlist_refused_before_step_up() {
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 10, 10);
    // Remove wallet.json on disk and check both auth:"stage" and auth:"approved"
    // from an unauthorized peer UID: peer UID check runs BEFORE examining the request.
    std::fs::remove_file(dir.path().join("wallet.json")).unwrap();
    let baseline = std::fs::read(&policy_path).unwrap();

    let caller_uid = rustix::process::geteuid().as_raw();
    let other_uid = caller_uid.wrapping_add(42_424);
    let (_sock_dir, sock_path, srv) = spawn_s3_socket_server(signer, vec![other_uid]).await;

    for auth in ["stage", "approved"] {
        let req = serde_json::json!({
            "op": "policy_update",
            "auth": auth,
            "policy": { "daily_cap_pct": 88 },
        });
        let resp = send_raw_socket_json(&sock_path, &req.to_string()).await;
        assert!(!resp.ok);
        assert_eq!(resp.code.as_deref(), Some("unauthorized"));
        assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);
    }
    assert!(!dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME)
        .exists());

    srv.abort();
}

#[test]
fn test_cli_policy_set_and_confirm_unsafe_refuse_unreachable_socket_without_password_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let unused_sock = tmp.path().join("unused.sock");
    let bin = env!("CARGO_BIN_EXE_vaughan");

    for args in [
        vec![
            "--profile",
            "sentient",
            "policy",
            "set",
            "--daily-cap-pct",
            "50",
        ],
        vec!["--profile", "sentient", "policy", "confirm-unsafe"],
    ] {
        let out = std::process::Command::new(bin)
            .args(&args)
            .env("HOME", tmp.path())
            .env("VAUGHAN_SIGNER_SOCKET", &unused_sock)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "{args:?} must fail cleanly on unreachable signer socket, got status {:?}: {}",
            out.status,
            String::from_utf8_lossy(&out.stdout)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("vault password") && stderr.contains("signer socket"),
            "expected signer socket refusal without vault password prompt for {args:?}, got stderr: {stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// S3b: root-approved policy updates integration tests (negative first)
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn write_s3b_pending(store: &std::path::Path, policy: &AgentSessionPolicy) -> std::path::PathBuf {
    let now = vaughan_agent::dca::now_unix();
    vaughan_cli::signerd::stage_pending_policy_with_replace(store, policy, now, true).unwrap();
    store.join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME)
}

#[cfg(unix)]
fn write_s3b_approval(
    store: &std::path::Path,
    old: &AgentSessionPolicy,
    new: &AgentSessionPolicy,
    ttl_secs: u64,
) -> std::path::PathBuf {
    let now = vaughan_agent::dca::now_unix();
    write_s3b_pending(store, new);
    let approval = vaughan_cli::signerd::Approval {
        old_digest: old.digest(),
        new_digest: new.digest(),
        issued_at_unix: now,
        expires_at_unix: now.saturating_add(ttl_secs),
    };
    vaughan_cli::signerd::write_approval(store, &approval).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3b_negative_approval_verifications_fail_closed_and_indistinguishable() {
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 10, 10);
    signer.set_expected_approval_uid(current_uid);

    let baseline_bytes = std::fs::read(&policy_path).unwrap();
    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 55;
    let mut p_other = p0.clone();
    p_other.daily_cap_pct = 77;

    let approval_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_APPROVAL_FILENAME);
    let audit_path = dir.path().join(vaughan_agent::sentient::POLICY_AUDIT_JSONL);

    let approved_req = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p1,
    });

    // 1. No approval present -> step_up_failed, disk unchanged.
    let r_missing: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_req.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r_missing.ok);
    assert_eq!(r_missing.code.as_deref(), Some("step_up_failed"));
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);
    assert!(!audit_path.exists());

    // 2. Wrong new_digest (pending holds p1, approval minted for p_other, request submits p1) -> identical code+message, unlinked.
    write_s3b_pending(dir.path(), &p1);
    let now_case2 = vaughan_agent::dca::now_unix();
    vaughan_cli::signerd::write_approval(
        dir.path(),
        &vaughan_cli::signerd::Approval {
            old_digest: p0.digest(),
            new_digest: p_other.digest(),
            issued_at_unix: now_case2,
            expires_at_unix: now_case2 + 120,
        },
    )
    .unwrap();
    assert!(approval_path.exists());
    let r_wrong_digest: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_req.to_string(), current_uid.wrapping_add(1))
            .await,
    )
    .unwrap();
    assert_eq!(r_wrong_digest.code, r_missing.code);
    assert_eq!(r_wrong_digest.message, r_missing.message);
    assert!(
        !approval_path.exists(),
        "approval must be unlinked on failure"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);

    // 3. Corrupt approval JSON -> identical code+message, unlinked.
    std::fs::write(&approval_path, b"{not valid json").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&approval_path, std::fs::Permissions::from_mode(0o440)).unwrap();
    }
    let r_corrupt: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_req.to_string(), current_uid.wrapping_add(2))
            .await,
    )
    .unwrap();
    assert_eq!(r_corrupt.code, r_missing.code);
    assert_eq!(r_corrupt.message, r_missing.message);
    assert!(!approval_path.exists());
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);

    // 4. Expired approval -> identical code+message, unlinked.
    let now = vaughan_agent::dca::now_unix();
    let expired = vaughan_cli::signerd::Approval {
        old_digest: p0.digest(),
        new_digest: p1.digest(),
        issued_at_unix: now.saturating_sub(200),
        expires_at_unix: now.saturating_sub(1),
    };
    vaughan_cli::signerd::write_approval(dir.path(), &expired).unwrap();
    let r_expired: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_req.to_string(), current_uid.wrapping_add(3))
            .await,
    )
    .unwrap();
    assert_eq!(r_expired.code, r_missing.code);
    assert_eq!(r_expired.message, r_missing.message);
    assert!(!approval_path.exists());
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);

    // 5. Approval owned by wrong UID -> identical code+message, unlinked.
    write_s3b_approval(dir.path(), &p0, &p1, 120);
    signer.set_expected_approval_uid(current_uid.wrapping_add(1));
    let r_wrong_uid: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_req.to_string(), current_uid.wrapping_add(4))
            .await,
    )
    .unwrap();
    assert_eq!(r_wrong_uid.code, r_missing.code);
    assert_eq!(r_wrong_uid.message, r_missing.message);
    assert!(!approval_path.exists());
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);
    signer.set_expected_approval_uid(current_uid);

    // 6. Mode 0644 and mode 0666 -> identical code+message, unlinked.
    for (idx, bad_mode) in [0o644u32, 0o666u32].into_iter().enumerate() {
        write_s3b_approval(dir.path(), &p0, &p1, 120);
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&approval_path, std::fs::Permissions::from_mode(bad_mode))
                .unwrap();
        }
        let r_bad_mode: SignerResponse = serde_json::from_str(
            &signer
                .handle_line_with_uid(
                    &approved_req.to_string(),
                    current_uid.wrapping_add(5 + idx as u32),
                )
                .await,
        )
        .unwrap();
        assert_eq!(r_bad_mode.code, r_missing.code);
        assert_eq!(r_bad_mode.message, r_missing.message);
        assert!(!approval_path.exists());
        assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);
    }

    // 7. Symlink -> identical code+message, symlink unlinked, target untouched.
    let target_file = dir.path().join("approval-target.json");
    let valid_json = serde_json::to_string(&vaughan_cli::signerd::Approval {
        old_digest: p0.digest(),
        new_digest: p1.digest(),
        issued_at_unix: now,
        expires_at_unix: now + 120,
    })
    .unwrap();
    std::fs::write(&target_file, &valid_json).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target_file, std::fs::Permissions::from_mode(0o440)).unwrap();
    }
    std::os::unix::fs::symlink(&target_file, &approval_path).unwrap();
    let r_symlink: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_req.to_string(), current_uid.wrapping_add(7))
            .await,
    )
    .unwrap();
    assert_eq!(r_symlink.code, r_missing.code);
    assert_eq!(r_symlink.message, r_missing.message);
    assert!(
        std::fs::symlink_metadata(&approval_path).is_err(),
        "symlink itself must be unlinked"
    );
    assert!(target_file.exists(), "symlink target must remain untouched");
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);

    // 8. nlink == 2 (hardlink) -> identical code+message, approval_path unlinked.
    write_s3b_approval(dir.path(), &p0, &p1, 120);
    let extra_link = dir.path().join("approval-hardlink.json");
    std::fs::hard_link(&approval_path, &extra_link).unwrap();
    let r_hardlink: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_req.to_string(), current_uid.wrapping_add(8))
            .await,
    )
    .unwrap();
    assert_eq!(r_hardlink.code, r_missing.code);
    assert_eq!(r_hardlink.message, r_missing.message);
    assert!(!approval_path.exists());
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3b_old_digest_toctou_conflict_and_policy_guards_with_valid_approval() {
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 10, 10);
    signer.set_expected_approval_uid(current_uid);

    let approval_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_APPROVAL_FILENAME);
    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();

    // 1. old_digest mismatch (TOCTOU) => policy_conflict, approval unlinked, P1 remains on disk.
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 25;
    let mut p2 = p0.clone();
    p2.daily_cap_pct = 60;
    write_s3b_approval(dir.path(), &p0, &p2, 120);
    // Simulate concurrent on-disk update to P1 after approval was minted for P0 -> P2.
    vaughan_agent::sentient::save_policy(dir.path(), &p1).unwrap();
    let p1_bytes = std::fs::read(&policy_path).unwrap();

    let req_p2 = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p2,
    });
    let r_toctou: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&req_p2.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r_toctou.ok);
    assert_eq!(r_toctou.code.as_deref(), Some("policy_conflict"));
    assert!(!approval_path.exists());
    assert_eq!(std::fs::read(&policy_path).unwrap(), p1_bytes);

    // 2. enforcement=disabled without persisted acknowledge_unsafe=true still refused with valid approval.
    let mut p_disabled = p1.clone();
    p_disabled.enforcement = EnforcementMode::Disabled;
    p_disabled.acknowledge_unsafe = true;
    write_s3b_approval(dir.path(), &p1, &p_disabled, 120);
    let req_disabled = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p_disabled,
    });
    let r_disabled: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&req_disabled.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r_disabled.ok);
    assert_eq!(r_disabled.code.as_deref(), Some("policy_refused"));
    assert!(!approval_path.exists());
    assert_eq!(std::fs::read(&policy_path).unwrap(), p1_bytes);

    // 3. Invalid policy (daily_cap_pct: 101) refused with valid approval.
    let mut p_invalid = p1.clone();
    p_invalid.daily_cap_pct = 101;
    write_s3b_approval(dir.path(), &p1, &p_invalid, 120);
    let req_invalid = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p_invalid,
    });
    let r_invalid: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&req_invalid.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r_invalid.ok);
    assert_eq!(r_invalid.code.as_deref(), Some("policy_invalid"));
    assert!(!approval_path.exists());
    assert_eq!(std::fs::read(&policy_path).unwrap(), p1_bytes);
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3b_second_stage_pending_and_unauthorized_peer_before_consume() {
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 10, 10);
    signer.set_expected_approval_uid(current_uid);
    let baseline_bytes = std::fs::read(&policy_path).unwrap();

    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 40;
    let mut p2 = p0.clone();
    p2.daily_cap_pct = 75;

    // 1. First stage writes policy-pending.json and returns step_up_required.
    let stage1 = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p1,
        "expected_digest": p0.digest(),
    });
    let r1: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&stage1.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r1.ok);
    assert_eq!(r1.code.as_deref(), Some("step_up_required"));
    assert_eq!(
        r1.message.as_deref(),
        Some("run: sudo vaughan signerd approve")
    );
    assert_eq!(r1.policy_digest.as_deref(), Some(p1.digest().as_str()));
    let pending_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME);
    let pending_bytes = std::fs::read(&pending_path).unwrap();

    // 2. Second stage while unexpired replaces nothing and returns step_up_pending.
    let stage2 = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p2,
        "expected_digest": p0.digest(),
    });
    let r2: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&stage2.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r2.ok);
    assert_eq!(r2.code.as_deref(), Some("step_up_pending"));
    assert_eq!(std::fs::read(&pending_path).unwrap(), pending_bytes);
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);

    // 3. Unauthorized peer UID refused BEFORE approval file is examined or consumed.
    let approval_path = write_s3b_approval(dir.path(), &p0, &p1, 120);
    let other_uid = current_uid.wrapping_add(42_424);
    let (_sock_dir, sock_path, srv) = spawn_s3_socket_server(signer, vec![other_uid]).await;
    let approved_req = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p1,
    });
    let r_unauth = send_raw_socket_json(&sock_path, &approved_req.to_string()).await;
    assert!(!r_unauth.ok);
    assert_eq!(r_unauth.code.as_deref(), Some("unauthorized"));
    assert!(
        approval_path.exists(),
        "approval file MUST remain present when unauthorized peer UID is rejected"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);

    srv.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3b_positive_stage_approve_apply_replay_and_sign() {
    let dest = Address::from([0x33; 20]);
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, broadcaster, dir, _, policy_path) =
        setup_s3_store(vec![format!("{dest:#x}")], 5, 90);
    signer.set_expected_approval_uid(current_uid);
    signer.set_balance(None, U256::from(1_000));

    let (_sock_dir, sock_path, srv) = spawn_s3_socket_server(signer, vec![current_uid]).await;
    let client = vaughan_core::core::SignerClient::new(&sock_path);

    let intent = SignerIntent::Transfer {
        chain_id: TEST_CHAIN_ID,
        to: dest,
        token: None,
        amount: U256::from(100), // 10% of 1000: blocked by daily_cap_pct=5
    };
    let before_err = client.sign(&intent).await.unwrap_err();
    assert!(
        before_err.contains("spend_guard") || before_err.contains("daily cap"),
        "expected daily cap refusal before update, got: {before_err}"
    );
    assert!(broadcaster.get_last_tx().is_none());

    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 90;

    // 1. Stage via socket.
    let stage_req = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p1,
        "expected_digest": p0.digest(),
    });
    let stage_resp = send_raw_socket_json(&sock_path, &stage_req.to_string()).await;
    assert!(!stage_resp.ok);
    assert_eq!(stage_resp.code.as_deref(), Some("step_up_required"));
    assert_eq!(
        stage_resp.message.as_deref(),
        Some("run: sudo vaughan signerd approve")
    );
    assert_eq!(
        stage_resp.policy_digest.as_deref(),
        Some(p1.digest().as_str())
    );
    let pending_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME);
    assert!(pending_path.exists());

    // 2. Simulate root approve writing 0440 approval (tests skip chown).
    let approval_path = write_s3b_approval(dir.path(), &p0, &p1, 120);
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&approval_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o440);
    }

    // 3. Apply via auth:"approved".
    let approved_req = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p1,
    });
    let apply_resp = send_raw_socket_json(&sock_path, &approved_req.to_string()).await;
    assert!(apply_resp.ok, "expected ok, got: {apply_resp:?}");
    assert_eq!(
        apply_resp.policy_digest.as_deref(),
        Some(p1.digest().as_str())
    );

    // 4. Verify `op:"policy"`, 0600 permissions, audit log, approval & pending gone.
    let after_read = client.policy().await.unwrap();
    assert_eq!(
        after_read.policy_digest.as_deref(),
        Some(p1.digest().as_str())
    );
    assert_eq!(after_read.policy.as_ref().unwrap()["daily_cap_pct"], 90);
    let disk_policy = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    assert_eq!(disk_policy.daily_cap_pct, 90);
    assert_eq!(disk_policy.digest(), p1.digest());
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&policy_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(!approval_path.exists());
    assert!(!pending_path.exists());

    let audit_path = dir.path().join(vaughan_agent::sentient::POLICY_AUDIT_JSONL);
    let audit_text = std::fs::read_to_string(&audit_path).unwrap();
    let audit_line: serde_json::Value = serde_json::from_str(audit_text.trim()).unwrap();
    assert_eq!(audit_line["source"], "socket");
    assert_eq!(audit_line["field_changes"]["daily_cap_pct"]["from"], 5);
    assert_eq!(audit_line["field_changes"]["daily_cap_pct"]["to"], 90);

    // 5. Next sign call uses new cap immediately.
    let sign_resp = client.sign(&intent).await.unwrap();
    assert!(sign_resp.ok);
    assert!(sign_resp.tx_hash.is_some());
    assert!(broadcaster.get_last_tx().is_some());

    // 6. Replay after consume: even if policy is reset on disk to p0, replaying fails step_up_failed.
    vaughan_agent::sentient::save_policy(dir.path(), &p0).unwrap();
    let p0_bytes = std::fs::read(&policy_path).unwrap();
    let replay_resp = send_raw_socket_json(&sock_path, &approved_req.to_string()).await;
    assert!(!replay_resp.ok);
    assert_eq!(replay_resp.code.as_deref(), Some("step_up_failed"));
    assert_eq!(std::fs::read(&policy_path).unwrap(), p0_bytes);

    srv.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3b_tighten_only_does_not_consume_or_get_blocked_by_rate_limiter() {
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 20, 20);
    signer.set_expected_approval_uid(current_uid);

    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut p_tight = p0.clone();
    p_tight.daily_cap_pct = 5;
    let mut p_wide = p0.clone();
    p_wide.daily_cap_pct = 80;

    let uid_tight = 2001u32;

    // 1. 6 failed tighten-only requests must NOT consume limiter budget for uid_tight.
    let tight_no_approval = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p_tight,
    });
    for _ in 0..6 {
        let resp: SignerResponse = serde_json::from_str(
            &signer
                .handle_line_with_uid(&tight_no_approval.to_string(), uid_tight)
                .await,
        )
        .unwrap();
        assert_eq!(resp.code.as_deref(), Some("step_up_failed"));
    }

    // Subsequent widening request for uid_tight must NOT be rate-limited.
    let wide_no_approval = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p_wide,
    });
    let wide_first: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&wide_no_approval.to_string(), uid_tight)
            .await,
    )
    .unwrap();
    assert_eq!(
        wide_first.code.as_deref(),
        Some("step_up_failed"),
        "tighten-only failures must not consume the widening rate limit"
    );

    // 2. Exhaust uid_tight's limiter with 4 more failed widening attempts (total 5).
    for _ in 0..4 {
        let resp: SignerResponse = serde_json::from_str(
            &signer
                .handle_line_with_uid(&wide_no_approval.to_string(), uid_tight)
                .await,
        )
        .unwrap();
        assert_eq!(resp.code.as_deref(), Some("step_up_failed"));
    }

    // 6th widening attempt is rate-limited.
    let wide_blocked: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&wide_no_approval.to_string(), uid_tight)
            .await,
    )
    .unwrap();
    assert_eq!(wide_blocked.code.as_deref(), Some("step_up_rate_limited"));

    // Rate-limited even with a valid staged pending + root approval; nothing consumed.
    let wide_stage = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p_wide,
        "expected_digest": p0.digest(),
    });
    let _ = signer
        .handle_line_with_uid(&wide_stage.to_string(), uid_tight)
        .await;
    let appr = write_s3b_approval(dir.path(), &p0, &p_wide, 120);
    let pending_file = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME);
    let before = std::fs::read(&policy_path).unwrap();
    let r_with_appr: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&wide_no_approval.to_string(), uid_tight)
            .await,
    )
    .unwrap();
    assert_eq!(r_with_appr.code.as_deref(), Some("step_up_rate_limited"));
    assert!(
        appr.exists(),
        "rate-limited request must not consume the approval"
    );
    assert!(
        pending_file.exists(),
        "rate-limited request must not clear pending policy"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), before);
    std::fs::remove_file(&appr).unwrap();
    std::fs::remove_file(&pending_file).unwrap();

    // 3. Even while uid_tight is rate-limited for widening, another peer UID is unaffected.
    let other_uid_resp: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&wide_no_approval.to_string(), uid_tight + 1)
            .await,
    )
    .unwrap();
    assert_eq!(other_uid_resp.code.as_deref(), Some("step_up_failed"));

    // 4. Even while uid_tight is rate-limited for widening, a valid tighten-only update succeeds!
    write_s3b_approval(dir.path(), &p0, &p_tight, 120);
    let tight_resp: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&tight_no_approval.to_string(), uid_tight)
            .await,
    )
    .unwrap();
    assert!(
        tight_resp.ok,
        "tighten-only update must not be blocked by rate limiter: {tight_resp:?}"
    );
    let disk_after = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    assert_eq!(disk_after.daily_cap_pct, 5);
    assert!(std::fs::read(&policy_path).is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3b_corrupt_wallet_does_not_block_stage_approve_apply() {
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 10, 10);
    signer.set_expected_approval_uid(current_uid);

    // Corrupt wallet.json on disk.
    std::fs::write(dir.path().join("wallet.json"), b"{corrupt json").unwrap();
    let baseline = std::fs::read(&policy_path).unwrap();

    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 45;

    // 1. Omitting auth returns step_up_unsupported; legacy password key returns invalid_json.
    let no_auth_req = serde_json::json!({
        "op": "policy_update",
        "policy": p1,
        "expected_digest": p0.digest(),
    });
    let r_no_auth: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&no_auth_req.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r_no_auth.ok);
    assert_eq!(r_no_auth.code.as_deref(), Some(STEP_UP_UNSUPPORTED_CODE));
    assert_eq!(r_no_auth.message.as_deref(), Some(STEP_UP_UNSUPPORTED_MSG));
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);

    let pw_req = serde_json::json!({
        "op": "policy_update",
        "password": S3_TEST_PW,
        "policy": p1,
        "expected_digest": p0.digest(),
    });
    let r_pw: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&pw_req.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r_pw.ok);
    assert_eq!(r_pw.code.as_deref(), Some("invalid_json"));
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);

    // 2. Approval path (stage -> write_approval -> approved) succeeds without wallet.json.
    let stage_req = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p1,
        "expected_digest": p0.digest(),
    });
    let r_stage: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&stage_req.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert_eq!(r_stage.code.as_deref(), Some("step_up_required"));

    write_s3b_approval(dir.path(), &p0, &p1, 120);
    let approved_req = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p1,
    });
    let r_approved: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_req.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(r_approved.ok, "approval path must succeed: {r_approved:?}");
    let disk_policy = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    assert_eq!(disk_policy.daily_cap_pct, 45);
}

#[cfg(unix)]
#[test]
fn test_s3b_cli_approve_help_non_root_and_approve_with_io() {
    let bin = env!("CARGO_BIN_EXE_vaughan");

    // 1. `--help` text must warn: run from a clean console or run0, not an ordinary terminal tab.
    let help_out = std::process::Command::new(bin)
        .args(["signerd", "approve", "--help"])
        .output()
        .unwrap();
    assert!(help_out.status.success());
    let help_stdout = String::from_utf8_lossy(&help_out.stdout);
    assert!(
        help_stdout.contains("run from a clean console or run0, not an ordinary terminal tab"),
        "missing clean-console/run0 warning in --help: {help_stdout}"
    );

    // 2. Non-root CLI invocation refuses cleanly (when test runs non-root).
    let current_uid = rustix::process::geteuid().as_raw();
    let dir = tempfile::tempdir().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    if current_uid != 0 {
        let out = std::process::Command::new(bin)
            .args(["signerd", "approve", "--store"])
            .arg(dir.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("root") || stderr.contains("euid"),
            "expected root/euid refusal, got: {stderr}"
        );
    }

    // 3. approve_with_io verifies non-TTY refusal, y/N refusal, and y approval diff+digest rendering.
    let p0 = AgentSessionPolicy::default();
    vaughan_agent::sentient::save_policy(dir.path(), &p0).unwrap();
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 50;
    let now = 1_700_000_000u64;
    vaughan_cli::signerd::approval::stage_pending_policy(dir.path(), &p1, now).unwrap();

    // Non-TTY refused.
    let mut out_buf = Vec::new();
    let mut empty_in = std::io::Cursor::new(b"y\n");
    let non_tty_err = vaughan_cli::signerd::approval::approve_with_io(
        dir.path(),
        120,
        current_uid,
        current_uid,
        false,
        now,
        &mut empty_in,
        &mut out_buf,
    )
    .unwrap_err();
    assert!(non_tty_err.to_string().contains("TTY"));

    // Explicit 'n' aborts without creating approval file.
    out_buf.clear();
    let mut no_in = std::io::Cursor::new(b"n\n");
    let abort_err = vaughan_cli::signerd::approval::approve_with_io(
        dir.path(),
        120,
        current_uid,
        current_uid,
        true,
        now,
        &mut no_in,
        &mut out_buf,
    )
    .unwrap_err();
    assert!(abort_err.to_string().contains("aborted"));
    assert!(!dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_APPROVAL_FILENAME)
        .exists());

    // Explicit 'y' prints digests + diff and writes 0440 approval matching in-memory policy.
    out_buf.clear();
    let mut yes_in = std::io::Cursor::new(b"y\n");
    let written = vaughan_cli::signerd::approval::approve_with_io(
        dir.path(),
        120,
        current_uid,
        current_uid,
        true,
        now,
        &mut yes_in,
        &mut out_buf,
    )
    .unwrap();
    let rendered = String::from_utf8(out_buf).unwrap();
    assert!(rendered.contains(&p0.digest()));
    assert!(rendered.contains(&p1.digest()));
    assert!(rendered.contains("daily_cap_pct: 20 -> 50"));

    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&written).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o440);
    }
    let consumed =
        vaughan_cli::signerd::consume_approval(dir.path(), current_uid, now + 10).unwrap();
    assert_eq!(consumed.old_digest, p0.digest());
    assert_eq!(consumed.new_digest, p1.digest());
    assert_eq!(consumed.issued_at_unix, now);
    assert_eq!(consumed.expires_at_unix, now + 120);
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3b_1b_fix_b_junk_approved_does_not_burn_approval_and_missing_pending_refuses() {
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 20, 20);
    signer.set_expected_approval_uid(current_uid);

    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 55;
    let mut p2 = p0.clone();
    p2.daily_cap_pct = 75;

    let approval_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_APPROVAL_FILENAME);
    let pending_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME);

    // 1. Valid approval present, NO policy-pending.json on disk -> step_up_failed, approval untouched.
    let now = vaughan_agent::dca::now_unix();
    vaughan_cli::signerd::write_approval(
        dir.path(),
        &vaughan_cli::signerd::Approval {
            old_digest: p0.digest(),
            new_digest: p1.digest(),
            issued_at_unix: now,
            expires_at_unix: now + 120,
        },
    )
    .unwrap();
    let approval_bytes_no_pending = std::fs::read(&approval_path).unwrap();
    let policy_bytes_initial = std::fs::read(&policy_path).unwrap();

    let approved_p1 = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p1,
    });
    let r_no_pending: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_p1.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r_no_pending.ok);
    assert_eq!(r_no_pending.code.as_deref(), Some("step_up_failed"));
    assert_eq!(
        r_no_pending.message.as_deref(),
        Some("policy approval verification failed")
    );
    assert_eq!(
        std::fs::read(&approval_path).unwrap(),
        approval_bytes_no_pending,
        "approval file must remain untouched when pending file is missing"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), policy_bytes_initial);

    // 2. Stage P1 and mint valid approval P0 -> P1.
    let stage_p1 = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p1,
        "expected_digest": p0.digest(),
    });
    let r_stage: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&stage_p1.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert_eq!(r_stage.code.as_deref(), Some("step_up_required"));

    let now2 = vaughan_agent::dca::now_unix();
    vaughan_cli::signerd::write_approval(
        dir.path(),
        &vaughan_cli::signerd::Approval {
            old_digest: p0.digest(),
            new_digest: p1.digest(),
            issued_at_unix: now2,
            expires_at_unix: now2 + 120,
        },
    )
    .unwrap();
    let approval_bytes = std::fs::read(&approval_path).unwrap();
    let pending_bytes = std::fs::read(&pending_path).unwrap();

    // 3. Junk/different approved requests (wrong policy P2, malformed string, omitted policy)
    //    must all return step_up_failed WITHOUT burning policy-approval.json or changing disk.
    for (idx, bad_req) in [
        serde_json::json!({
            "op": "policy_update",
            "auth": "approved",
            "policy": p2,
        }),
        serde_json::json!({
            "op": "policy_update",
            "auth": "approved",
            "policy": "junk",
        }),
        serde_json::json!({
            "op": "policy_update",
            "auth": "approved",
        }),
    ]
    .into_iter()
    .enumerate()
    {
        let r_junk: SignerResponse = serde_json::from_str(
            &signer
                .handle_line_with_uid(
                    &bad_req.to_string(),
                    current_uid.wrapping_add(10 + idx as u32),
                )
                .await,
        )
        .unwrap();
        assert!(!r_junk.ok);
        assert_eq!(r_junk.code.as_deref(), Some("step_up_failed"));
        assert_eq!(
            r_junk.message.as_deref(),
            Some("policy approval verification failed")
        );
        assert_eq!(
            std::fs::read(&approval_path).unwrap(),
            approval_bytes,
            "junk approved request #{idx} must NOT burn approval file"
        );
        assert_eq!(std::fs::read(&pending_path).unwrap(), pending_bytes);
        assert_eq!(std::fs::read(&policy_path).unwrap(), policy_bytes_initial);
    }

    // 4. Genuine approved request with staged P1 succeeds, updates disk policy, and consumes both files.
    let r_ok: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&approved_p1.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(
        r_ok.ok,
        "genuine approved P1 must succeed after junk attempts: {r_ok:?}"
    );
    let disk_after = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    assert_eq!(disk_after.daily_cap_pct, 55);
    assert_eq!(disk_after.digest(), p1.digest());
    assert!(!approval_path.exists());
    assert!(!pending_path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3b_1b_fix_a_tightening_replaces_held_pending_slot_and_deletes_approval() {
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 20, 20);
    signer.set_expected_approval_uid(current_uid);

    let baseline_bytes = std::fs::read(&policy_path).unwrap();
    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let pending_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME);
    let approval_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_APPROVAL_FILENAME);

    // 1. Attacker holds the pending slot with non-tightening P_x (daily_cap_pct: 50).
    let mut p_x = p0.clone();
    p_x.daily_cap_pct = 50;
    let stage_px = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p_x,
        "expected_digest": p0.digest(),
    });
    let r_px: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&stage_px.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert_eq!(r_px.code.as_deref(), Some("step_up_required"));
    let pending_px_bytes = std::fs::read(&pending_path).unwrap();

    // 2. Second non-tightening stage P_w (daily_cap_pct: 60) -> step_up_pending, disk byte-identical.
    let mut p_w = p0.clone();
    p_w.daily_cap_pct = 60;
    let stage_pw = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p_w,
        "expected_digest": p0.digest(),
    });
    let r_pw: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&stage_pw.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert_eq!(r_pw.code.as_deref(), Some("step_up_pending"));
    assert_eq!(std::fs::read(&pending_path).unwrap(), pending_px_bytes);

    // 3. Negative (mixed caps): lowering daily_cap_pct (10 < 20) while raising max_position_pct (25 > 20)
    //    is NOT tighten-only -> step_up_pending, pending unchanged.
    let mut p_mixed = p0.clone();
    p_mixed.daily_cap_pct = 10;
    p_mixed.max_position_pct = 25;
    let stage_mixed = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p_mixed,
        "expected_digest": p0.digest(),
    });
    let r_mixed: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&stage_mixed.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert_eq!(r_mixed.code.as_deref(), Some("step_up_pending"));
    assert_eq!(std::fs::read(&pending_path).unwrap(), pending_px_bytes);

    // 4. Negative (Invariant 5): tighter than held pending P_x (30 < 50) but wider than current on-disk P0 (30 > 20)
    //    must NOT replace held pending slot.
    let mut p_vs_pending = p0.clone();
    p_vs_pending.daily_cap_pct = 30;
    let stage_vs_pending = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p_vs_pending,
        "expected_digest": p0.digest(),
    });
    let r_vs_pending: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&stage_vs_pending.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert_eq!(r_vs_pending.code.as_deref(), Some("step_up_pending"));
    assert_eq!(std::fs::read(&pending_path).unwrap(), pending_px_bytes);

    // 5. Place an existing approval minted for P_x on disk, then human stages tightening P_t (10 < 20).
    let now = vaughan_agent::dca::now_unix();
    vaughan_cli::signerd::write_approval(
        dir.path(),
        &vaughan_cli::signerd::Approval {
            old_digest: p0.digest(),
            new_digest: p_x.digest(),
            issued_at_unix: now,
            expires_at_unix: now + 120,
        },
    )
    .unwrap();
    assert!(approval_path.exists());

    let mut p_t = p0.clone();
    p_t.daily_cap_pct = 10;
    let stage_pt = serde_json::json!({
        "op": "policy_update",
        "auth": "stage",
        "policy": p_t,
        "expected_digest": p0.digest(),
    });
    let r_pt: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&stage_pt.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r_pt.ok);
    assert_eq!(r_pt.code.as_deref(), Some("step_up_required"));
    assert_eq!(r_pt.policy_digest.as_deref(), Some(p_t.digest().as_str()));

    let disk_pending: vaughan_cli::signerd::approval::PendingPolicy =
        serde_json::from_str(&std::fs::read_to_string(&pending_path).unwrap()).unwrap();
    assert_eq!(disk_pending.policy, p_t);
    assert!(
        !approval_path.exists(),
        "replacing held pending slot must delete any stale policy-approval.json"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);
}

#[cfg(unix)]
#[tokio::test]
async fn test_s3c4_approved_wrong_expected_digest_does_not_burn_approval() {
    let current_uid = rustix::process::geteuid().as_raw();
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 20, 20);
    signer.set_expected_approval_uid(current_uid);

    let p0 = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    let mut p1 = p0.clone();
    p1.daily_cap_pct = 55;

    let approval_path = write_s3b_approval(dir.path(), &p0, &p1, 120);
    let pending_path = dir
        .path()
        .join(vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME);

    let approval_bytes = std::fs::read(&approval_path).unwrap();
    let pending_bytes = std::fs::read(&pending_path).unwrap();
    let policy_bytes = std::fs::read(&policy_path).unwrap();

    // 1. Request with a wrong expected_digest must return policy_conflict WITHOUT burning the approval.
    let bad_req = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p1,
        "expected_digest": "0000000000000000000000000000000000000000000000000000000000000000",
    });
    let r_bad: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&bad_req.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(!r_bad.ok);
    assert_eq!(r_bad.code.as_deref(), Some("policy_conflict"));
    assert_eq!(
        std::fs::read(&approval_path).unwrap(),
        approval_bytes,
        "wrong expected_digest must NOT burn policy-approval.json"
    );
    assert_eq!(
        std::fs::read(&pending_path).unwrap(),
        pending_bytes,
        "wrong expected_digest must NOT remove policy-pending.json"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), policy_bytes);

    // 2. Same request with the right expected_digest succeeds using the unburned approval.
    let ok_req = serde_json::json!({
        "op": "policy_update",
        "auth": "approved",
        "policy": p1,
        "expected_digest": p0.digest(),
    });
    let r_ok: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&ok_req.to_string(), current_uid)
            .await,
    )
    .unwrap();
    assert!(
        r_ok.ok,
        "request with matching expected_digest must succeed after rejected wrong-digest attempt: {r_ok:?}"
    );
    assert_eq!(r_ok.policy_digest.as_deref(), Some(p1.digest().as_str()));
    assert_eq!(
        vaughan_agent::sentient::load_policy(dir.path()).unwrap(),
        p1
    );
    assert!(!approval_path.exists());
    assert!(!pending_path.exists());
}
