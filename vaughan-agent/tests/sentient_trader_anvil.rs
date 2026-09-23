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
}

const TARGET: Address = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9");
const ONE_ETH: u64 = 1_000_000_000_000_000_000;

#[tokio::test]
async fn test_sentient_trader_autonomous_execution_with_anvil() {
    let anvil = AnvilGuard::spawn(8557);
    plant_stub_contract(&anvil.rpc_url, TARGET).await;
    let trader = trader_for(anvil.rpc_url.clone());

    let outcome = trader
        .execute_swap(
            TARGET,
            None,
            Bytes::new(),
            U256::from(ONE_ETH),
            U256::from(ONE_ETH),
            50,
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
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("no contract code"), "{err}");
}
