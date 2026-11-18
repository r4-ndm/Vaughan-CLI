//! Integration tests for the daemon's durable kill switch and per-request policy
//! freshness (gas-ceiling-and-trip spec, Design B + A4; tests 12-13, mock
//! broadcaster only, no anvil). Each guard here is mutation-checked: remove it,
//! this file's matching test goes red.

use std::sync::Arc;

use alloy::primitives::{Address, U256};
use secrecy::SecretString;
use vaughan_agent::sentient::policy::{AgentSessionPolicy, EnforcementMode};
use vaughan_cli::signerd::{
    MockBalanceSource, MockBroadcaster, MockSwapQuoter, Signer, SignerResponse, TRIP_FILE,
};
use vaughan_core::core::{OperatingMode, SignerIntent, WalletState};
use vaughan_core::security::hd_wallet::validate_mnemonic;

const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";
const TEST_CHAIN_ID: u64 = 943; // PulseChain testnet v4
const PEER_UID: u32 = 1000;

/// A signer over `store`, creating the wallet there on first call. A second call with the
/// same `store` models a daemon RESTART against the same on-disk state.
fn signer_for_store(
    store: &std::path::Path,
    allow_transfer_to: Vec<String>,
) -> (Signer, Arc<MockBroadcaster>) {
    let wallet_path = store.join("wallet.json");
    let existed = wallet_path.is_file();
    let mut wallet =
        WalletState::load_with_session(wallet_path, OperatingMode::SentientTrader, "sentient")
            .unwrap();
    let password = SecretString::from("Password123!@#".to_string());
    if !existed {
        wallet
            .create(&password, validate_mnemonic(TEST_MNEMONIC).unwrap())
            .unwrap();
    }
    wallet.unlock(&password).unwrap();
    wallet.set_active_network("pulsechain-testnet-v4").unwrap();
    let active_addr = wallet.active_address().unwrap().parse::<Address>().unwrap();

    if !existed {
        let policy = AgentSessionPolicy {
            enforcement: EnforcementMode::Enforced,
            daily_cap_pct: 100,
            max_position_pct: 100,
            max_slippage_bps: 9_999,
            allow_transfer_to,
            ..Default::default()
        };
        vaughan_agent::sentient::save_policy(store, &policy).unwrap();
    }

    let broadcaster = Arc::new(MockBroadcaster::new());
    let balances = Arc::new(MockBalanceSource::new());
    balances.set_default(U256::from(1_000_000_000_000u64));
    let signer = Signer::new(
        store.to_path_buf(),
        active_addr,
        TEST_CHAIN_ID,
        "http://127.0.0.1:8545".to_string(),
        Arc::new(MockSwapQuoter::new()),
        broadcaster.clone(),
        balances.clone(),
    );
    (signer, broadcaster)
}

async fn trip(signer: &Signer, reason: &str) -> SignerResponse {
    let req = serde_json::json!({ "op": "trip", "reason": reason });
    serde_json::from_str(
        &signer
            .handle_line_with_uid(&req.to_string(), PEER_UID)
            .await,
    )
    .unwrap()
}

async fn sign_transfer(signer: &Signer, to: Address) -> SignerResponse {
    let intent = SignerIntent::Transfer {
        chain_id: TEST_CHAIN_ID,
        to,
        token: None,
        amount: U256::from(100u64),
    };
    let req = serde_json::json!({ "op": "sign", "intent": intent });
    serde_json::from_str(
        &signer
            .handle_line_with_uid(&req.to_string(), PEER_UID)
            .await,
    )
    .unwrap()
}

#[tokio::test]
async fn test_trip_op_refuses_later_signs_and_sanitises_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster) = signer_for_store(dir.path(), vec![format!("{dest:#x}")]);

    let resp = trip(&signer, "\u{1b}[2J\noperator stop").await;
    assert!(resp.ok, "{resp:?}");
    let msg = resp.message.unwrap();
    assert!(
        !msg.contains('\u{1b}') && !msg.contains('\n'),
        "control characters must be stripped before the reason is formatted: {msg:?}"
    );
    assert!(msg.contains("kill switch: [2Joperator stop"), "{msg}");

    let refused = sign_transfer(&signer, dest).await;
    assert!(!refused.ok);
    assert_eq!(refused.code.as_deref(), Some("circuit_breaker_tripped"));
    assert!(refused.message.unwrap().contains("[2Joperator stop"));
    assert!(
        broadcaster.get_last_tx().is_none(),
        "a tripped daemon must not reach the broadcaster"
    );
}

#[tokio::test]
async fn test_trip_survives_a_restart_with_the_original_reason() {
    let dir = tempfile::tempdir().unwrap();
    let dest = Address::repeat_byte(0x11);
    {
        let (signer, _) = signer_for_store(dir.path(), vec![format!("{dest:#x}")]);
        assert!(trip(&signer, "operator stop").await.ok);
    }

    // A fresh Signer over the same store is a restarted daemon: a kill switch that only
    // lived in the old process's memory would come back signing.
    let (restarted, broadcaster) = signer_for_store(dir.path(), vec![format!("{dest:#x}")]);
    let refused = sign_transfer(&restarted, dest).await;
    assert!(!refused.ok);
    assert_eq!(refused.code.as_deref(), Some("circuit_breaker_tripped"));
    assert!(
        refused.message.unwrap().contains("operator stop"),
        "the restarted daemon must report the ORIGINAL reason"
    );
    assert!(broadcaster.get_last_tx().is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn test_trip_file_is_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (signer, _) = signer_for_store(dir.path(), vec![]);
    assert!(trip(&signer, "mode check").await.ok);

    let mode = std::fs::metadata(dir.path().join(TRIP_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "the trip marker records why trading halted: owner-only"
    );
}

#[tokio::test]
async fn test_second_trip_keeps_the_first_reason_in_memory_and_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let (signer, _) = signer_for_store(dir.path(), vec![]);

    // An automatic trip (what the gas ceiling / error streak raise) lands first.
    signer
        .breaker()
        .trip("automatic: consecutive error tripwire");
    let resp = trip(&signer, "manual second").await;
    assert!(resp.ok, "{resp:?}");

    let reason = signer.breaker().trip_reason().unwrap();
    assert!(
        reason.contains("automatic"),
        "first reason must win: got {reason}"
    );
    let on_disk = std::fs::read_to_string(dir.path().join(TRIP_FILE)).unwrap();
    assert!(
        on_disk.contains("automatic") && !on_disk.contains("manual second"),
        "the persisted reason must be the one that actually halted trading: {on_disk}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_racing_automatic_and_manual_trips_agree_on_one_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (signer, _) = signer_for_store(dir.path(), vec![]);

    let breaker = signer.breaker();
    let automatic = tokio::task::spawn_blocking(move || {
        breaker.trip("automatic: gas ceiling exceeded");
    });
    let manual = {
        let signer = signer.clone();
        tokio::spawn(async move { trip(&signer, "manual kill").await })
    };
    automatic.await.unwrap();
    assert!(manual.await.unwrap().ok);

    // Whichever landed first, memory and disk must tell the operator the SAME story.
    let reason = signer.breaker().trip_reason().unwrap();
    let on_disk = std::fs::read_to_string(dir.path().join(TRIP_FILE)).unwrap();
    let marker = if reason.contains("automatic") {
        "automatic"
    } else {
        "manual kill"
    };
    assert!(
        on_disk.contains(marker),
        "in-memory reason {reason:?} disagrees with the persisted {on_disk}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_corrupt_trip_file_starts_the_daemon_tripped() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let dest = Address::repeat_byte(0x11);
    // Create the wallet/policy first, then plant an unparseable marker.
    let (_warmup, _) = signer_for_store(dir.path(), vec![format!("{dest:#x}")]);
    let path = dir.path().join(TRIP_FILE);
    std::fs::write(&path, b"{ truncated").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let (signer, broadcaster) = signer_for_store(dir.path(), vec![format!("{dest:#x}")]);
    let refused = sign_transfer(&signer, dest).await;
    assert!(
        !refused.ok,
        "a trip marker that cannot be read must fail CLOSED, not silently clear"
    );
    assert_eq!(refused.code.as_deref(), Some("circuit_breaker_tripped"));
    assert!(broadcaster.get_last_tx().is_none());
}

#[tokio::test]
async fn test_automatic_trip_during_sign_is_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster) = signer_for_store(dir.path(), vec![format!("{dest:#x}")]);
    broadcaster.set_fail(true);
    broadcaster.set_fail_phase(vaughan_cli::signerd::SendPhase::Definitive);

    // Default max_consecutive_errors is 3: the third definitive failure trips the breaker.
    for _ in 0..3 {
        assert!(!sign_transfer(&signer, dest).await.ok);
    }
    assert!(signer.breaker().is_tripped());
    assert!(
        dir.path().join(TRIP_FILE).is_file(),
        "an automatic trip must be persisted too, or a restart resumes trading"
    );

    let (restarted, _) = signer_for_store(dir.path(), vec![format!("{dest:#x}")]);
    assert_eq!(
        sign_transfer(&restarted, dest).await.code.as_deref(),
        Some("circuit_breaker_tripped")
    );
}

#[tokio::test]
async fn test_trip_is_recorded_in_the_policy_audit_log() {
    let dir = tempfile::tempdir().unwrap();
    let (signer, _) = signer_for_store(dir.path(), vec![]);
    assert!(trip(&signer, "audited stop").await.ok);

    let audit =
        std::fs::read_to_string(dir.path().join(vaughan_agent::sentient::POLICY_AUDIT_JSONL))
            .expect("a trip must leave a trace in the audit log");
    let last: serde_json::Value = serde_json::from_str(audit.lines().last().unwrap()).unwrap();
    assert_eq!(last["event"], "circuit_breaker_tripped");
    assert_eq!(last["peer_uid"], PEER_UID);
    assert_eq!(last["reason"], "audited stop");
}

/// Test 13: the policy is re-read per request, so a ceiling tightened on disk must bind the
/// very next sign — the breaker's config is otherwise only set in `Signer::new`.
#[tokio::test]
async fn test_policy_edit_on_disk_tightens_the_next_sign() {
    let dir = tempfile::tempdir().unwrap();
    let dest = Address::repeat_byte(0x11);
    let dest2 = Address::repeat_byte(0x22);
    let (signer, broadcaster) = signer_for_store(
        dir.path(),
        vec![format!("{dest:#x}"), format!("{dest2:#x}")],
    );
    broadcaster.set_quote_max_fee(U256::from(1));
    broadcaster.set_quote_gas_limit(21_000);

    let first = sign_transfer(&signer, dest).await;
    assert!(first.ok, "{first:?}");

    // Tighten the ceiling below one tx's quoted fee, exactly as an operator hand-edit would.
    let mut policy = vaughan_agent::sentient::load_policy(dir.path()).unwrap();
    policy.max_session_gas_wei = "1".to_string();
    vaughan_agent::sentient::save_policy(dir.path(), &policy).unwrap();

    // A different destination so the refusal is the ceiling, not the spend ledger's
    // content-derived replay guard.
    let refused = sign_transfer(&signer, dest2).await;
    assert!(!refused.ok);
    assert_eq!(refused.code.as_deref(), Some("gas_ceiling_exceeded"));
    assert!(refused.message.unwrap().contains("max_session_gas_wei"));
}

/// A5: `status` reports the gas ceiling in force plus what is left of it.
#[tokio::test]
async fn test_status_reports_gas_budget_keys() {
    let dir = tempfile::tempdir().unwrap();
    let dest = Address::repeat_byte(0x11);
    let (signer, broadcaster) = signer_for_store(dir.path(), vec![format!("{dest:#x}")]);
    broadcaster.set_quote_max_fee(U256::from(1));
    broadcaster.set_quote_gas_limit(21_000);
    assert!(sign_transfer(&signer, dest).await.ok);

    let resp: SignerResponse =
        serde_json::from_str(&signer.handle_line(r#"{"op":"status"}"#).await).unwrap();
    let budget = resp.budget.unwrap();
    assert_eq!(budget["gas_spent_wei"], "21000");
    let max: U256 = budget["gas_max_wei"].as_str().unwrap().parse().unwrap();
    let remaining: U256 = budget["gas_remaining_wei"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(remaining, max - U256::from(21_000));
}
