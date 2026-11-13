//! Deterministic autonomous trader tests against a local Anvil node.

use alloy::network::Ethereum;
use alloy::primitives::{address, Address, Bytes, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::signers::local::PrivateKeySigner;
use std::process::{Child, Command};
use std::time::Duration;

use vaughan_agent::sentient::{CircuitBreakerConfig, SentientTrader};

struct AnvilGuard {
    child: Child,
    rpc_url: String,
}

impl AnvilGuard {
    fn spawn(port: u16) -> Self {
        let child = Command::new("anvil")
            .args(["-p", &port.to_string(), "--silent"])
            .spawn()
            .expect("Failed to start Anvil.");

        std::thread::sleep(Duration::from_millis(400));
        let rpc_url = format!("http://127.0.0.1:{}", port);
        Self { child, rpc_url }
    }
}

impl Drop for AnvilGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Plant a one-byte `STOP` contract so the trader's empty-code guard passes.
async fn plant_stub_contract(rpc_url: &str, at: Address) {
    let provider: RootProvider<Ethereum> = RootProvider::new_http(rpc_url.parse().unwrap());
    let _: () = provider
        .raw_request("anvil_setCode".into(), (at, "0x00"))
        .await
        .expect("anvil_setCode");
}

fn trader_for(rpc_url: String) -> SentientTrader {
    let burner_signer: PrivateKeySigner =
        "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
            .parse()
            .unwrap();
    SentientTrader::new(
        "test",
        burner_signer,
        vec![rpc_url],
        31337,
        CircuitBreakerConfig {
            max_position_pct: 50,
            max_slippage_bps: 100,
            max_session_gas_wei: U256::from(10_000_000_000_000_000u64),
            max_consecutive_errors: 3,
            required_rpc_quorum: 1,
            ..Default::default()
        },
    )
    .unwrap()
}

const TARGET: Address = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9");
const ONE_ETH: u64 = 1_000_000_000_000_000_000;

#[tokio::test]
async fn test_sentient_trader_autonomous_execution_with_anvil() {
    let anvil = AnvilGuard::spawn(8557);
    plant_stub_contract(&anvil.rpc_url, TARGET).await;
    let dir = tempfile::tempdir().unwrap();
    let trader = trader_for(anvil.rpc_url.clone()).with_profile_dir(dir.path());

    let outcome = trader
        .execute_swap(
            TARGET,
            None,
            Bytes::new(),
            U256::from(ONE_ETH),
            U256::from(ONE_ETH),
            50,
            None,
            None,
        )
        .await
        .unwrap();

    assert!(!outcome.dry_run);
    assert!(!outcome.tx_hash.is_zero());
    assert!(!trader.circuit_breaker().is_tripped());
}

#[tokio::test]
async fn test_sentient_trader_dry_run_skips_broadcast() {
    let anvil = AnvilGuard::spawn(8558);
    plant_stub_contract(&anvil.rpc_url, TARGET).await;
    let trader = trader_for(anvil.rpc_url.clone()).with_dry_run(true);
    assert!(trader.is_dry_run());

    let before = trader.circuit_breaker().is_tripped();
    let outcome = trader
        .execute_swap(
            TARGET,
            None,
            Bytes::new(),
            U256::from(ONE_ETH),
            U256::from(ONE_ETH),
            50,
            None,
            None,
        )
        .await
        .unwrap();

    assert!(outcome.dry_run);
    assert!(outcome.tx_hash.is_zero());
    assert_eq!(before, trader.circuit_breaker().is_tripped());
}

#[tokio::test]
async fn test_sentient_trader_refuses_codeless_router() {
    let anvil = AnvilGuard::spawn(8562);
    let trader = trader_for(anvil.rpc_url.clone());

    let err = trader
        .execute_swap(
            TARGET,
            None,
            Bytes::new(),
            U256::from(ONE_ETH),
            U256::from(ONE_ETH),
            50,
            None,
            None,
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("no contract code"), "{err}");
}

fn mock_router_bytecode(amount_in: u64, amount_out: u64) -> Vec<u8> {
    let mut code = Vec::new();
    // Calldata selector: PUSH1 0x00; CALLDATALOAD; PUSH1 0xe0; SHR
    code.extend_from_slice(&[0x60, 0x00, 0x35, 0x60, 0xe0, 0x1c]);
    // PUSH4 0xd06ca61f (getAmountsOut)
    code.extend_from_slice(&[0x63, 0xd0, 0x6c, 0xa6, 0x1f]);
    // EQ
    code.push(0x14);
    // PUSH1 0x10; JUMPI
    code.extend_from_slice(&[0x60, 0x10, 0x57]);
    // STOP for other calls (e.g. swapExactETHForTokens)
    code.push(0x00);

    // JUMPDEST at 0x10 (16)
    assert_eq!(code.len(), 0x10);
    code.push(0x5b);

    // Word 0: 0x20 (offset of dynamic array)
    let mut w0 = [0u8; 32];
    w0[31] = 0x20;
    code.push(0x7f);
    code.extend_from_slice(&w0);
    code.extend_from_slice(&[0x60, 0x00, 0x52]);

    // Word 1: 0x02 (array length 2)
    let mut w1 = [0u8; 32];
    w1[31] = 0x02;
    code.push(0x7f);
    code.extend_from_slice(&w1);
    code.extend_from_slice(&[0x60, 0x20, 0x52]);

    // Word 2: amount_in
    let mut w2 = [0u8; 32];
    w2[24..32].copy_from_slice(&amount_in.to_be_bytes());
    code.push(0x7f);
    code.extend_from_slice(&w2);
    code.extend_from_slice(&[0x60, 0x40, 0x52]);

    // Word 3: amount_out
    let mut w3 = [0u8; 32];
    w3[24..32].copy_from_slice(&amount_out.to_be_bytes());
    code.push(0x7f);
    code.extend_from_slice(&w3);
    code.extend_from_slice(&[0x60, 0x60, 0x52]);

    // RETURN(0x00, 0x80)
    code.extend_from_slice(&[0x60, 0x80, 0x60, 0x00, 0xf3]);

    code
}

async fn plant_contract_code(rpc_url: &str, at: Address, code: &[u8]) {
    let provider: RootProvider<Ethereum> = RootProvider::new_http(rpc_url.parse().unwrap());
    let hex_code = format!("0x{}", hex::encode(code));
    let _: () = provider
        .raw_request("anvil_setCode".into(), (at, hex_code))
        .await
        .expect("anvil_setCode");
}

#[tokio::test]
async fn test_dca_slice_fire_reserves_ledger() {
    let port = 8563;
    let child = Command::new("anvil")
        .args([
            "-p",
            &port.to_string(),
            "--chain-id",
            "369",
            "--block-base-fee-per-gas",
            "0",
            "--silent",
        ])
        .spawn()
        .expect("Failed to start Anvil.");
    std::thread::sleep(Duration::from_millis(400));
    let rpc_url = format!("http://127.0.0.1:{}", port);
    let _guard = AnvilGuard {
        child,
        rpc_url: rpc_url.clone(),
    };

    let router = TARGET; // 0x165C3410fC91EF562C50559f7d2289fEbed552d9 (PulseX V2 router)
    let router_code = mock_router_bytecode(1000, 2000);
    plant_contract_code(&rpc_url, router, &router_code).await;

    let dir = tempfile::tempdir().unwrap();

    let before = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    assert!(before.assets.is_empty());

    let burner_signer: PrivateKeySigner =
        "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
            .parse()
            .unwrap();
    let trader = SentientTrader::new(
        "test",
        burner_signer.clone(),
        vec![rpc_url],
        369,
        CircuitBreakerConfig {
            max_position_pct: 100,
            max_slippage_bps: 500,
            max_session_gas_wei: U256::from(10_000_000_000_000_000u64),
            max_consecutive_errors: 3,
            required_rpc_quorum: 1,
            ..Default::default()
        },
    )
    .unwrap();

    let plan_id = "dca_test_plan";
    let now = 1_700_000_000u64;
    let plan = vaughan_agent::dca::DcaPlan {
        id: plan_id.into(),
        chain_id: 369,
        account: format!("{:#x}", trader.address()),
        token_out: "0x2222222222222222222222222222222222222222".into(),
        slice_amount_wei: "1000".into(),
        trigger: vaughan_agent::dca::DcaTrigger::Time {
            interval_secs: 3600,
        },
        venue: vaughan_agent::dca::DcaVenue::Dex {
            name: "pulsex".into(),
            protocol: "v2".into(),
        },
        max_slippage_bps: 500,
        max_slices: 10,
        slices_done: 0,
        spent_wei: "0".into(),
        consecutive_failures: 0,
        status: vaughan_agent::dca::DcaStatus::Active,
        created_at: now - 3600,
        next_due_at: now,
        dry_run: false,
        slice_log: vec![],
        pair: None,
    };
    vaughan_agent::dca::add_plan(dir.path(), plan).unwrap();

    let result = vaughan_agent::dca::fire_plan(dir.path(), plan_id, &trader, 369, None, None, now)
        .await
        .unwrap();

    assert!(result.ok, "DCA fire should succeed: {:?}", result.error);
    assert!(!result.tx_hash.is_empty());

    let after = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    let native = after
        .assets
        .get("369:native")
        .expect("asset recorded in ledger");
    assert_eq!(native.spent, U256::from(1000));
    assert_eq!(after.recent.len(), 1);
    assert_eq!(after.recent[0].key, format!("dca_{plan_id}_0"));
}

#[tokio::test]
async fn test_trader_refuses_with_no_profile_dir() {
    let anvil = AnvilGuard::spawn(8564);
    plant_stub_contract(&anvil.rpc_url, TARGET).await;
    let trader = trader_for(anvil.rpc_url.clone());
    assert!(trader.profile_dir().is_none());

    let err = trader
        .execute_swap(
            TARGET,
            None,
            Bytes::new(),
            U256::from(ONE_ETH),
            U256::from(ONE_ETH),
            50,
            None,
            None,
        )
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("spend_ledger_unreadable: sentient trader has no profile dir bound"),
        "expected profile dir bound refusal, got: {msg}"
    );
}

#[tokio::test]
async fn test_trader_refuses_on_unreadable_policy() {
    let anvil = AnvilGuard::spawn(8565);
    plant_stub_contract(&anvil.rpc_url, TARGET).await;
    let dir = tempfile::tempdir().unwrap();
    let policy_path = dir.path().join("sentient-policy.toml");
    std::fs::write(&policy_path, "invalid toml {[[[[").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&policy_path, std::fs::Permissions::from_mode(0o600));
    }

    let trader = trader_for(anvil.rpc_url.clone()).with_profile_dir(dir.path());

    let err = trader
        .execute_swap(
            TARGET,
            None,
            Bytes::new(),
            U256::from(ONE_ETH),
            U256::from(ONE_ETH),
            50,
            None,
            None,
        )
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("invalid sentient-policy.toml") || msg.contains("failed to read"),
        "expected unreadable policy error, got: {msg}"
    );
}

#[tokio::test]
async fn test_token_in_swap_reserves_under_token_key_not_router() {
    let anvil = AnvilGuard::spawn(8566);
    plant_stub_contract(&anvil.rpc_url, TARGET).await;
    let dir = tempfile::tempdir().unwrap();

    let token_in = alloy::primitives::address!("0x2222222222222222222222222222222222222222");
    let trader = trader_for(anvil.rpc_url.clone()).with_profile_dir(dir.path());
    let token_amount = U256::from(500);
    let token_balance = U256::from(10_000);

    let key = "token_swap_test_1";
    let outcome = trader
        .execute_swap_keyed(
            TARGET,
            None,
            Bytes::new(),
            U256::ZERO,
            token_amount,
            50,
            Some(token_in),
            Some(token_balance),
            Some(key),
        )
        .await
        .unwrap();

    assert!(!outcome.dry_run);
    assert!(!outcome.tx_hash.is_zero());

    let snap = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    let token_bucket = format!("31337:{token_in:#x}");
    let router_bucket = format!("31337:{TARGET:#x}");

    assert!(
        snap.assets.contains_key(&token_bucket),
        "expected ledger to contain token bucket {token_bucket}, got {:?}",
        snap.assets.keys()
    );
    assert!(
        !snap.assets.contains_key(&router_bucket),
        "ledger must NOT reserve under router bucket {router_bucket}"
    );
    assert_eq!(snap.assets.get(&token_bucket).unwrap().spent, token_amount);
}

#[tokio::test]
async fn test_trader_second_swap_with_same_key_does_not_broadcast() {
    let anvil = AnvilGuard::spawn(8567);
    plant_stub_contract(&anvil.rpc_url, TARGET).await;
    let dir = tempfile::tempdir().unwrap();
    let trader = trader_for(anvil.rpc_url.clone()).with_profile_dir(dir.path());
    let key = "replay_swap_key_1";

    let outcome1 = trader
        .execute_swap_keyed(
            TARGET,
            None,
            Bytes::new(),
            U256::from(ONE_ETH),
            U256::from(ONE_ETH),
            50,
            None,
            None,
            Some(key),
        )
        .await
        .unwrap();
    assert!(!outcome1.tx_hash.is_zero());

    // Second swap with the same key must be refused and NOT broadcast
    let err2 = trader
        .execute_swap_keyed(
            TARGET,
            None,
            Bytes::new(),
            U256::from(ONE_ETH),
            U256::from(ONE_ETH),
            50,
            None,
            None,
            Some(key),
        )
        .await
        .unwrap_err();

    let msg = err2.to_string();
    assert!(
        msg.contains("spend_already_reserved:"),
        "expected spend_already_reserved, got: {msg}"
    );

    // Ledger spent total should still be exactly ONE_ETH, not doubled
    let snap = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
    assert_eq!(
        snap.assets.get("31337:native").unwrap().spent,
        U256::from(ONE_ETH)
    );
}
