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
// S3: policy & policy_update password step-up integration tests
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
async fn test_policy_update_auth_failures_and_corrupt_store_indistinguishable() {
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 5, 50);
    let baseline = std::fs::read(&policy_path).unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let (_sock_dir, sock_path, srv) = spawn_s3_socket_server(signer, vec![uid]).await;

    let client = vaughan_core::core::SignerClient::new(&sock_path);
    let current = client.policy().await.unwrap();
    let cur_digest = current.policy_digest.clone();
    let mut modified = current.policy.unwrap();
    modified["daily_cap_pct"] = serde_json::json!(90);

    // 1. No password field at all -> step_up_failed, store unchanged, no audit file.
    let no_pw_req = serde_json::json!({
        "op": "policy_update",
        "policy": modified,
        "expected_digest": cur_digest,
    });
    let r_no_pw = send_raw_socket_json(&sock_path, &no_pw_req.to_string()).await;
    assert!(!r_no_pw.ok);
    assert_eq!(r_no_pw.code.as_deref(), Some("step_up_failed"));
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);
    assert!(!dir
        .path()
        .join(vaughan_agent::sentient::POLICY_AUDIT_JSONL)
        .exists());

    // 2. Empty password -> step_up_failed, store unchanged.
    let err_empty = client
        .policy_update(
            secrecy::SecretString::from(String::new()),
            modified.clone(),
            cur_digest.clone(),
        )
        .await
        .unwrap_err();
    assert!(err_empty.starts_with("step_up_failed:"));
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);

    // 3. Wrong password -> step_up_failed with generic verification message.
    let wrong_pw_req = serde_json::json!({
        "op": "policy_update",
        "password": "WrongPassword!999",
        "policy": modified,
        "expected_digest": cur_digest,
    });
    let r_wrong = send_raw_socket_json(&sock_path, &wrong_pw_req.to_string()).await;
    assert!(!r_wrong.ok);
    assert_eq!(r_wrong.code.as_deref(), Some("step_up_failed"));
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);

    // 4. Corrupt wallet.json -> indistinguishable response from wrong password.
    let (corrupt_signer, _, corrupt_dir, _, corrupt_policy) = setup_s3_store(vec![], 5, 50);
    let corrupt_baseline = std::fs::read(&corrupt_policy).unwrap();
    std::fs::write(corrupt_dir.path().join("wallet.json"), b"{corrupt json").unwrap();
    let (_sock_dir2, sock_path2, srv2) = spawn_s3_socket_server(corrupt_signer, vec![uid]).await;
    let right_pw_req = serde_json::json!({
        "op": "policy_update",
        "password": S3_TEST_PW,
        "policy": modified,
    });
    let r_corrupt = send_raw_socket_json(&sock_path2, &right_pw_req.to_string()).await;
    assert!(!r_corrupt.ok);
    assert_eq!(r_corrupt.code, r_wrong.code);
    assert_eq!(r_corrupt.message, r_wrong.message);
    assert_eq!(std::fs::read(&corrupt_policy).unwrap(), corrupt_baseline);

    // 5. Missing wallet.json -> indistinguishable response too.
    std::fs::remove_file(corrupt_dir.path().join("wallet.json")).unwrap();
    let r_missing = send_raw_socket_json(&sock_path2, &right_pw_req.to_string()).await;
    assert_eq!(r_missing.code, r_wrong.code);
    assert_eq!(r_missing.message, r_wrong.message);

    srv.abort();
    srv2.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn test_policy_update_right_password_loopback_takes_effect_on_next_sign() {
    let dest = Address::from([0x22; 20]);
    let (signer, broadcaster, dir, _, policy_path) =
        setup_s3_store(vec![format!("{dest:#x}")], 5, 90);
    signer.set_balance(None, U256::from(1_000));
    let uid = rustix::process::geteuid().as_raw();
    let (_sock_dir, sock_path, srv) = spawn_s3_socket_server(signer, vec![uid]).await;

    let client = vaughan_core::core::SignerClient::new(&sock_path);
    let intent = SignerIntent::Transfer {
        chain_id: TEST_CHAIN_ID,
        to: dest,
        token: None,
        amount: U256::from(100), // 10% of 1000: allowed by max_position_pct=90, blocked by daily_cap_pct=5
    };

    // 1. Before update: refused by daily_cap_pct = 5% (50 wei).
    let before_err = client.sign(&intent).await.unwrap_err();
    assert!(
        before_err.contains("spend_guard") || before_err.contains("daily cap"),
        "expected daily cap refusal before step-up, got: {before_err}"
    );
    assert!(broadcaster.get_last_tx().is_none());

    // 2. Step-up policy_update raising daily_cap_pct 5 -> 90.
    let current = client.policy().await.unwrap();
    let mut new_policy = current.policy.unwrap();
    new_policy["daily_cap_pct"] = serde_json::json!(90);
    let update_resp = client
        .policy_update(
            secrecy::SecretString::from(S3_TEST_PW.to_string()),
            new_policy,
            current.policy_digest,
        )
        .await
        .unwrap();
    assert!(update_resp.ok);
    let new_digest = update_resp.policy_digest.clone().unwrap();

    // 3. `op:"policy"` returns new values and matching digest; file on disk matches.
    let after_read = client.policy().await.unwrap();
    assert_eq!(
        after_read.policy_digest.as_deref(),
        Some(new_digest.as_str())
    );
    assert_eq!(after_read.policy.as_ref().unwrap()["daily_cap_pct"], 90);
    let disk_policy = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    assert_eq!(disk_policy.daily_cap_pct, 90);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&policy_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // 4. Audit log records old/new values and never logs password.
    let audit_path = dir.path().join(vaughan_agent::sentient::POLICY_AUDIT_JSONL);
    let audit_text = std::fs::read_to_string(&audit_path).unwrap();
    assert!(!audit_text.contains(S3_TEST_PW));
    let audit_line: serde_json::Value = serde_json::from_str(audit_text.trim()).unwrap();
    assert_eq!(audit_line["source"], "socket");
    assert_eq!(audit_line["field_changes"]["daily_cap_pct"]["from"], 5);
    assert_eq!(audit_line["field_changes"]["daily_cap_pct"]["to"], 90);

    // 5. Loopback: very next `sign` on the SAME daemon session succeeds immediately!
    let sign_resp = client.sign(&intent).await.unwrap();
    assert!(sign_resp.ok);
    assert!(sign_resp.tx_hash.is_some());
    assert!(broadcaster.get_last_tx().is_some());

    srv.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn test_policy_update_refusals_and_two_step_disable_leave_store_unchanged() {
    let (signer, _, _dir, _, policy_path) = setup_s3_store(vec![], 10, 10);
    let uid = rustix::process::geteuid().as_raw();
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
                    "password": S3_TEST_PW,
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
                    "password": S3_TEST_PW,
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
                    "password": S3_TEST_PW,
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
                    "password": S3_TEST_PW,
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
                "password": S3_TEST_PW,
                "policy": base_val,
                "expected_digest": "0000000000000000000000000000000000000000000000000000000000000000",
            }),
            "policy_conflict",
        ),
        (
            "unknown top-level request field",
            serde_json::json!({
                "op": "policy_update",
                "password": S3_TEST_PW,
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
                    "password": S3_TEST_PW,
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
    }

    // Two-step disable works once acknowledge_unsafe=true is already persisted on disk.
    let mut step1 = base_val.clone();
    step1["acknowledge_unsafe"] = serde_json::json!(true);
    let r1 = client
        .policy_update(
            secrecy::SecretString::from(S3_TEST_PW.to_string()),
            step1.clone(),
            Some(cur_digest),
        )
        .await
        .unwrap();
    assert!(r1.ok);
    let mut step2 = step1;
    step2["enforcement"] = serde_json::json!("disabled");
    let r2 = client
        .policy_update(
            secrecy::SecretString::from(S3_TEST_PW.to_string()),
            step2,
            r1.policy_digest,
        )
        .await
        .unwrap();
    assert!(r2.ok);

    srv.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn test_policy_update_audit_rotates_at_200_lines_over_socket() {
    use std::os::unix::fs::PermissionsExt;

    let (signer, _, dir, _, _) = setup_s3_store(vec![], 10, 10);
    let audit_path = dir.path().join(vaughan_agent::sentient::POLICY_AUDIT_JSONL);
    let mut seeded = String::new();
    for i in 0..200 {
        seeded.push_str(&format!(
            "{{\"ts\":{i},\"field_changes\":{{}},\"source\":\"socket\"}}\n"
        ));
    }
    std::fs::write(&audit_path, seeded).unwrap();
    std::fs::set_permissions(&audit_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let uid = rustix::process::geteuid().as_raw();
    let (_sock_dir, sock_path, srv) = spawn_s3_socket_server(signer, vec![uid]).await;
    let client = vaughan_core::core::SignerClient::new(&sock_path);
    let current = client.policy().await.unwrap();
    let mut updated = current.policy.unwrap();
    updated["daily_cap_pct"] = serde_json::json!(42);

    let resp = client
        .policy_update(
            secrecy::SecretString::from(S3_TEST_PW.to_string()),
            updated,
            current.policy_digest,
        )
        .await
        .unwrap();
    assert!(resp.ok);

    let after = std::fs::read_to_string(&audit_path).unwrap();
    assert!(!after.contains(S3_TEST_PW));
    let lines: Vec<&str> = after.lines().collect();
    assert_eq!(lines.len(), 200);
    let oldest: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(oldest["ts"], 1);
    let newest: serde_json::Value = serde_json::from_str(lines[199]).unwrap();
    assert_eq!(newest["field_changes"]["daily_cap_pct"]["from"], 10);
    assert_eq!(newest["field_changes"]["daily_cap_pct"]["to"], 42);

    srv.abort();
}

#[tokio::test]
async fn test_policy_update_rate_limits_after_5_wrong_passwords() {
    let (signer, _, _dir, _, policy_path) = setup_s3_store(vec![], 10, 10);
    let baseline = std::fs::read(&policy_path).unwrap();
    let mut new_policy = vaughan_agent::sentient::AgentSessionPolicy::default();
    new_policy.daily_cap_pct = 50;
    let new_val = serde_json::to_value(&new_policy).unwrap();

    let wrong_req = serde_json::json!({
        "op": "policy_update",
        "password": "BadPassword!123",
        "policy": new_val,
    });
    let uid_a = 1000;
    for attempt in 1..=5 {
        let resp: SignerResponse = serde_json::from_str(
            &signer
                .handle_line_with_uid(&wrong_req.to_string(), uid_a)
                .await,
        )
        .unwrap();
        assert_eq!(
            resp.code.as_deref(),
            Some("step_up_failed"),
            "attempt {attempt} should fail auth"
        );
    }

    // 6th attempt within 60s window MUST be rate-limited even with the RIGHT password!
    let right_req = serde_json::json!({
        "op": "policy_update",
        "password": S3_TEST_PW,
        "policy": new_val,
    });
    let blocked: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&right_req.to_string(), uid_a)
            .await,
    )
    .unwrap();
    assert!(!blocked.ok);
    assert_eq!(blocked.code.as_deref(), Some("step_up_rate_limited"));
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);

    // A different peer UID is tracked independently and is NOT rate-limited.
    let other_uid_resp: SignerResponse = serde_json::from_str(
        &signer
            .handle_line_with_uid(&wrong_req.to_string(), uid_a + 1)
            .await,
    )
    .unwrap();
    assert_eq!(other_uid_resp.code.as_deref(), Some("step_up_failed"));
}

#[cfg(unix)]
#[tokio::test]
async fn test_policy_update_peer_uid_outside_allowlist_refused_before_password_check() {
    let (signer, _, dir, _, policy_path) = setup_s3_store(vec![], 10, 10);
    // Remove wallet.json on disk: if password check ran, it would return step_up_failed;
    // because peer UID is checked BEFORE examining the password/request, it must return
    // `unauthorized` instead (and leave step_up_limiter untouched).
    std::fs::remove_file(dir.path().join("wallet.json")).unwrap();
    let baseline = std::fs::read(&policy_path).unwrap();

    let caller_uid = rustix::process::geteuid().as_raw();
    let other_uid = caller_uid.wrapping_add(42_424);
    let (_sock_dir, sock_path, srv) = spawn_s3_socket_server(signer, vec![other_uid]).await;

    let req = serde_json::json!({
        "op": "policy_update",
        "password": S3_TEST_PW,
        "policy": { "daily_cap_pct": 88 },
    });
    let resp = send_raw_socket_json(&sock_path, &req.to_string()).await;
    assert!(!resp.ok);
    assert_eq!(resp.code.as_deref(), Some("unauthorized"));
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);

    srv.abort();
}

#[test]
fn test_cli_policy_set_and_confirm_unsafe_refuse_non_tty_stdin() {
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
            "{args:?} must refuse non-TTY stdin, got status {:?}: {}",
            out.status,
            String::from_utf8_lossy(&out.stdout)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("TTY"),
            "expected TTY refusal message for {args:?}, got stderr: {stderr}"
        );
    }
}
