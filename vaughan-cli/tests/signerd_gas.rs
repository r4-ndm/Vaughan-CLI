//! Integration tests for the daemon's gas ceiling and metered broadcast
//! (gas-ceiling-and-trip spec, Design A / tests 1-11; mock broadcaster only,
//! no anvil). Each guard here is mutation-checked: remove it, this file's
//! matching test goes red.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{Address, U256};
use secrecy::SecretString;
use vaughan_agent::sentient::policy::{AgentSessionPolicy, EnforcementMode};
use vaughan_agent::sentient::CircuitBreakerConfig;
use vaughan_cli::signerd::{
    MockBalanceSource, MockBroadcaster, MockSwapQuoter, SendPhase, Signer, SignerResponse,
};
use vaughan_core::core::{BatchTransferLeg, OperatingMode, SignerIntent, SwapFamily, WalletState};
use vaughan_core::security::hd_wallet::validate_mnemonic;

const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";
const TEST_CHAIN_ID: u64 = 943; // PulseChain testnet v4
const V3_ROUTER: &str = "0xfC656c95eCd418536844FeeaA46949bb9365BEaF";

fn setup_test_harness(
    allow_transfer_to: Vec<String>,
) -> (
    Signer,
    Arc<MockBroadcaster>,
    Arc<MockSwapQuoter>,
    tempfile::TempDir,
    Address,
    Arc<MockBalanceSource>,
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
        max_position_pct: 100,
        max_slippage_bps: 9_999,
        allow_transfer_to,
        ..Default::default()
    };
    vaughan_agent::sentient::save_policy(dir.path(), &policy).unwrap();

    let broadcaster = Arc::new(MockBroadcaster::new());
    let quoter = Arc::new(MockSwapQuoter::new());
    let balances = Arc::new(MockBalanceSource::new());
    balances.set_default(U256::from(1_000_000_000_000u64));
    let rpc_url = "http://127.0.0.1:8545".to_string();

    let signer = Signer::new(
        dir.path().to_path_buf(),
        active_addr,
        TEST_CHAIN_ID,
        rpc_url,
        quoter.clone(),
        broadcaster.clone(),
        balances.clone(),
    );

    (signer, broadcaster, quoter, dir, active_addr, balances)
}

/// Sends one `sign` request for `intent` and returns the parsed daemon response.
async fn sign_intent(signer: &Signer, intent: &SignerIntent) -> SignerResponse {
    let req = serde_json::json!({ "op": "sign", "intent": intent });
    serde_json::from_str(&signer.handle_line(&req.to_string()).await).unwrap()
}

/// Set the session gas ceiling the way an operator would: on disk. `handle_sign` reloads the
/// policy per request and reapplies it to the breaker (spec A4), so a ceiling set only in
/// memory would be overwritten by the first sign.
fn set_ceiling(store: &std::path::Path, signer: &Signer, max_session_gas_wei: U256) {
    let mut policy = vaughan_agent::sentient::load_policy(store).unwrap();
    policy.max_session_gas_wei = max_session_gas_wei.to_string();
    vaughan_agent::sentient::save_policy(store, &policy).unwrap();
    // Also in memory, so a test can assert on the ceiling before any sign happens.
    signer.breaker().replace_config(CircuitBreakerConfig {
        max_session_gas_wei,
        ..signer.breaker().config()
    });
}

fn transfer(to: Address, amount: u64) -> SignerIntent {
    SignerIntent::Transfer {
        chain_id: TEST_CHAIN_ID,
        to,
        token: None,
        amount: U256::from(amount),
    }
}

#[tokio::test]
async fn test_over_ceiling_quote_refused_and_nothing_sent() {
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster, _, _dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    // Default mock quote is 21000 * 1 gwei = 2.1e13 wei; a 1-wei ceiling cannot admit it.
    set_ceiling(_dir.path(), &signer, U256::from(1));

    let resp = sign_intent(&signer, &transfer(dest, 100)).await;
    assert!(!resp.ok);
    assert_eq!(resp.code.as_deref(), Some("gas_ceiling_exceeded"));
    assert!(resp.message.unwrap().contains("max_session_gas_wei"));
    assert!(
        broadcaster.get_last_tx().is_none(),
        "an over-ceiling quote must never reach the broadcaster"
    );
}

#[tokio::test]
async fn test_gas_refusal_releases_ledger_legs_for_retry() {
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster, _, _dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    set_ceiling(_dir.path(), &signer, U256::from(1));

    let intent = transfer(dest, 100);
    let refused = sign_intent(&signer, &intent).await;
    assert!(!refused.ok);
    assert_eq!(refused.code.as_deref(), Some("gas_ceiling_exceeded"));

    // Raise the ceiling (equivalent to a cheaper quote) and retry the SAME intent: it must
    // succeed, proving the ledger legs from the refused attempt were released.
    set_ceiling(_dir.path(), &signer, U256::MAX);
    broadcaster.set_quote_max_fee(U256::from(1));
    let ok = sign_intent(&signer, &intent).await;
    assert!(ok.ok, "{ok:?}");
    assert!(ok.tx_hash.is_some());
}

#[tokio::test]
async fn test_gas_refusal_does_not_trip_or_count_as_error() {
    let dest = Address::repeat_byte(0x11);
    let (signer, _, _, _dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    set_ceiling(_dir.path(), &signer, U256::from(1));

    for _ in 0..3 {
        let resp = sign_intent(&signer, &transfer(dest, 100)).await;
        assert_eq!(resp.code.as_deref(), Some("gas_ceiling_exceeded"));
    }

    assert!(!signer.breaker().is_tripped());
    assert_eq!(signer.breaker().consecutive_errors(), 0);
}

#[tokio::test]
async fn test_cumulative_gas_second_tx_refused_boundary_admitted() {
    let dest1 = Address::repeat_byte(0x11);
    let dest2 = Address::repeat_byte(0x22);
    let dest3 = Address::repeat_byte(0x33);
    let (signer, broadcaster, _, _dir, _, _) = setup_test_harness(vec![
        format!("{dest1:#x}"),
        format!("{dest2:#x}"),
        format!("{dest3:#x}"),
    ]);
    // Fixed 21000-wei quote per tx (21000 gas * 1 wei/gas); ceiling admits exactly two.
    broadcaster.set_quote_max_fee(U256::from(1));
    broadcaster.set_quote_gas_limit(21_000);
    set_ceiling(_dir.path(), &signer, U256::from(42_000));

    let r1 = sign_intent(&signer, &transfer(dest1, 10)).await;
    assert!(r1.ok, "{r1:?}");
    assert_eq!(signer.breaker().cumulative_gas_wei(), U256::from(21_000));

    // Boundary: projected 21000 + 21000 == 42000 ceiling is admitted (`>` trips, not `>=`).
    let r2 = sign_intent(&signer, &transfer(dest2, 20)).await;
    assert!(r2.ok, "{r2:?}");
    assert_eq!(signer.breaker().cumulative_gas_wei(), U256::from(42_000));

    // A third tx would push to 63000 > 42000: refused.
    let r3 = sign_intent(&signer, &transfer(dest3, 30)).await;
    assert!(!r3.ok);
    assert_eq!(r3.code.as_deref(), Some("gas_ceiling_exceeded"));
    assert_eq!(signer.breaker().cumulative_gas_wei(), U256::from(42_000));
}

#[tokio::test]
async fn test_definitive_broadcast_failure_releases_gas_and_counts_error() {
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster, _, _dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    broadcaster.set_fail(true);
    broadcaster.set_fail_phase(SendPhase::Definitive);

    let resp = sign_intent(&signer, &transfer(dest, 100)).await;
    assert!(!resp.ok);
    assert_eq!(resp.code.as_deref(), Some("broadcast_failed"));
    assert_eq!(
        signer.breaker().cumulative_gas_wei(),
        U256::ZERO,
        "a definitive failure must release its gas reservation"
    );
    assert_eq!(signer.breaker().consecutive_errors(), 1);

    // Ledger legs released too: the same intent succeeds once the broadcaster stops failing.
    broadcaster.set_fail(false);
    let ok = sign_intent(&signer, &transfer(dest, 100)).await;
    assert!(ok.ok, "{ok:?}");
}

#[tokio::test]
async fn test_uncertain_broadcast_failure_keeps_gas_and_legs() {
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster, _, _dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    broadcaster.set_fail(true);
    broadcaster.set_fail_phase(SendPhase::Uncertain);

    let intent = transfer(dest, 100);
    let resp = sign_intent(&signer, &intent).await;
    assert!(!resp.ok);
    assert_eq!(resp.code.as_deref(), Some("broadcast_uncertain"));
    assert!(
        !signer.breaker().cumulative_gas_wei().is_zero(),
        "an uncertain failure must KEEP its gas reservation"
    );
    assert_eq!(
        signer.breaker().consecutive_errors(),
        0,
        "an uncertain failure must not count as an error"
    );

    // The ledger leg is kept too: an identical retry must not be re-armed.
    let retry = sign_intent(&signer, &intent).await;
    assert!(!retry.ok);
    assert_eq!(retry.code.as_deref(), Some("spend_already_reserved"));
}

#[tokio::test]
async fn test_quote_failure_and_validation_floor_refuse_as_gas_unavailable() {
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster, _, _dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);

    // Quote failure: nothing sent, legs released (retry after clearing succeeds).
    broadcaster.set_quote_fail(true, "rpc down");
    let intent = transfer(dest, 100);
    let resp = sign_intent(&signer, &intent).await;
    assert!(!resp.ok);
    assert_eq!(resp.code.as_deref(), Some("gas_unavailable"));
    assert!(broadcaster.get_last_tx().is_none());
    broadcaster.set_quote_fail(false, "");
    assert!(sign_intent(&signer, &intent).await.ok);

    // gas_limit below the 21000 floor.
    let (signer2, broadcaster2, _, _dir2, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    broadcaster2.set_quote_gas_limit(20_999);
    let resp = sign_intent(&signer2, &transfer(dest, 1)).await;
    assert_eq!(resp.code.as_deref(), Some("gas_unavailable"));

    // Zero max_fee_per_gas must never read as "free".
    let (signer3, broadcaster3, _, _dir3, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    broadcaster3.set_quote_max_fee(U256::ZERO);
    let resp = sign_intent(&signer3, &transfer(dest, 1)).await;
    assert_eq!(resp.code.as_deref(), Some("gas_unavailable"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_concurrent_reservations_never_exceed_ceiling() {
    let addrs: Vec<Address> = (1u8..=10).map(Address::repeat_byte).collect();
    let allow: Vec<String> = addrs.iter().map(|a| format!("{a:#x}")).collect();
    let (signer, broadcaster, _, _dir, _, _) = setup_test_harness(allow);
    // 21000 gas * 1 wei/gas = 21000 wei per tx; a 105000-wei ceiling admits exactly 5 of 10.
    broadcaster.set_quote_max_fee(U256::from(1));
    broadcaster.set_quote_gas_limit(21_000);
    set_ceiling(_dir.path(), &signer, U256::from(105_000));

    let mut handles = Vec::new();
    for (i, addr) in addrs.into_iter().enumerate() {
        let signer = signer.clone();
        handles.push(tokio::spawn(async move {
            sign_intent(&signer, &transfer(addr, 100 + i as u64)).await
        }));
    }
    let mut admitted = 0u32;
    for h in handles {
        if h.await.unwrap().ok {
            admitted += 1;
        }
    }

    // Upper bound only (not exact equality): under heavy parallel-test-harness contention,
    // unrelated ledger-file I/O can make fewer than 5 succeed, but reserve_gas's single
    // write-lock scope must never let the total admitted exceed the ceiling.
    assert!(
        admitted <= 5,
        "reserve_gas must be atomic: at most 5 of 10 concurrent 21000-wei txs may be \
         admitted under a 105000-wei ceiling (check-then-add could admit more): got {admitted}"
    );
    assert!(signer.breaker().cumulative_gas_wei() <= U256::from(105_000));
}

#[tokio::test]
async fn test_trip_race_during_quote_stops_the_send_and_releases_gas() {
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster, _, _dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    broadcaster.arm_quote_gate();

    let signer2 = signer.clone();
    let intent = transfer(dest, 100);
    let task = tokio::spawn(async move { sign_intent(&signer2, &intent).await });

    // Let the spawned sign reach the gate before tripping and releasing it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    signer.breaker().trip("race");
    broadcaster.release_quote_gate();

    let resp = task.await.unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code.as_deref(), Some("circuit_breaker_tripped"));
    assert!(
        broadcaster.get_last_tx().is_none(),
        "a trip that lands during quote/reserve must stop the send"
    );
    assert_eq!(signer.breaker().cumulative_gas_wei(), U256::ZERO);
}

#[tokio::test]
async fn test_saturating_max_fee_refuses_without_panic() {
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster, _, _dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);
    broadcaster.set_quote_max_fee(U256::MAX);

    let resp = sign_intent(&signer, &transfer(dest, 1)).await;
    assert!(!resp.ok);
    assert_eq!(resp.code.as_deref(), Some("gas_ceiling_exceeded"));
}

#[tokio::test]
async fn test_every_op_kind_is_metered() {
    let dest = Address::repeat_byte(0x11);
    let router = Address::from_str(V3_ROUTER).unwrap();
    let token_in = Address::repeat_byte(0x22);
    let token_out = Address::repeat_byte(0x33);
    let pair = Address::repeat_byte(0x99);

    let (signer, broadcaster, quoter, _dir, _active, balances) =
        setup_test_harness(vec![format!("{dest:#x}")]);
    quoter.set_v3(pair, U256::from(1_000_000));
    balances.set_default(U256::from(1_000_000_000_000u64));
    // Over any sane ceiling: the default breaker ceiling is 5e16 wei.
    broadcaster.set_quote_max_fee(U256::MAX);

    let intents: Vec<(&str, SignerIntent)> = vec![
        ("transfer", transfer(dest, 100)),
        (
            "swap",
            SignerIntent::Swap {
                chain_id: TEST_CHAIN_ID,
                family: SwapFamily::V3,
                router,
                path: vec![token_in, token_out],
                amount_in: U256::from(1_000),
                // Slippage floor: 1,000,000 quote * (10000-9999)/10000 == 100.
                min_amount_out: U256::from(100),
                fee: Some(3000),
            },
        ),
        (
            "approve",
            SignerIntent::Approve {
                chain_id: TEST_CHAIN_ID,
                token: token_in,
                spender: router,
                amount: U256::from(1_000),
                expiry: None,
            },
        ),
        (
            "wrap",
            SignerIntent::Wrap {
                chain_id: TEST_CHAIN_ID,
                amount: U256::from(1_000),
            },
        ),
        (
            "unwrap",
            SignerIntent::Unwrap {
                chain_id: TEST_CHAIN_ID,
                amount: U256::from(1_000),
            },
        ),
        (
            "batch_transfer",
            SignerIntent::BatchTransfer {
                chain_id: TEST_CHAIN_ID,
                legs: vec![BatchTransferLeg {
                    to: dest,
                    amount: U256::from(1_000),
                }],
            },
        ),
        (
            "dca",
            SignerIntent::DcaSlice {
                plan_id: "plan_metered".into(),
                slice_index: 0,
                chain_id: TEST_CHAIN_ID,
                pair,
                amount_in: U256::from(1_000),
                corridor_min_out: U256::from(1),
            },
        ),
    ];

    for (name, intent) in intents {
        let resp = sign_intent(&signer, &intent).await;
        assert!(!resp.ok, "{name}: must be refused by the gas ceiling");
        assert_eq!(
            resp.code.as_deref(),
            Some("gas_ceiling_exceeded"),
            "{name}: {resp:?}"
        );
    }
    assert!(
        broadcaster.get_last_tx().is_none(),
        "no metered op may reach the broadcaster over the ceiling"
    );
}

/// Source guard for test 11: every op handler must go through `broadcast_metered` /
/// `quote_fee` via `gas.rs`, not call the broadcaster directly — `sweep.rs` is the sole,
/// deliberate exception (its fee is paid from the swept note, not the session gas budget).
#[test]
fn test_ops_source_files_do_not_bypass_metering() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/signerd/ops");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("sweep.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("broadcaster.broadcast"),
            "{}: must call broadcast_metered, not the broadcaster directly",
            path.display()
        );
        assert!(
            !text.contains("quote_fee("),
            "{}: must call broadcast_metered, not quote_fee directly",
            path.display()
        );
    }
}

/// A store whose policy file went missing must stop signing, not sign under the
/// built-in defaults (`load_policy` answers `Ok(default)` for absence, and the
/// defaults can be far looser than the ceilings the daemon was started under).
#[tokio::test]
async fn test_missing_policy_file_refuses_to_sign() {
    let dest = Address::repeat_byte(0x11);
    let (signer, _, _, dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);

    // Sanity: with the policy present the same intent signs.
    let ok = sign_intent(&signer, &transfer(dest, 100)).await;
    assert!(ok.ok, "{ok:?}");

    std::fs::remove_file(dir.path().join("sentient-policy.toml")).unwrap();
    let resp = sign_intent(&signer, &transfer(dest, 100)).await;
    assert!(!resp.ok);
    assert_eq!(
        resp.code.as_deref(),
        Some("policy_load_failed"),
        "a missing policy file must fail closed, not install the defaults"
    );
}

/// The per-request reload must go through the session-aware builder: a policy whose
/// `required_rpc_quorum` exceeds the single configured endpoint is clamped, exactly as
/// at startup, instead of silently over-promising multi-RPC verification.
#[tokio::test]
async fn test_policy_reload_keeps_the_rpc_quorum_clamp() {
    let dest = Address::repeat_byte(0x22);
    let (signer, _, _, dir, _, _) = setup_test_harness(vec![format!("{dest:#x}")]);

    let mut policy = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    policy.required_rpc_quorum = 3; // one endpoint configured; startup clamps to 1
    vaughan_agent::sentient::save_policy(dir.path(), &policy).unwrap();

    let ok = sign_intent(&signer, &transfer(dest, 100)).await;
    assert!(ok.ok, "{ok:?}");
    assert_eq!(
        signer.breaker().config().required_rpc_quorum,
        1,
        "the reload must clamp the quorum to the endpoint count, like Signer::new"
    );
}
