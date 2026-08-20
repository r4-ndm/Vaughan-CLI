//! Deterministic autonomous trader tests against a local Anvil node.

use alloy::primitives::{address, Bytes, U256};
use alloy::signers::local::PrivateKeySigner;
use std::process::{Child, Command};
use std::time::Duration;

use vaughan_agent::degen::{CircuitBreakerConfig, DegenTrader};

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

#[tokio::test]
async fn test_degen_trader_autonomous_execution_with_anvil() {
    let anvil = AnvilGuard::spawn(8557);
    let rpc_url = anvil.rpc_url.clone();

    let burner_signer: PrivateKeySigner =
        "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
            .parse()
            .unwrap();

    let burner_addr = burner_signer.address();

    let trader = DegenTrader::new(
        burner_signer,
        vec![rpc_url.clone()],
        31337,
        CircuitBreakerConfig {
            max_position_pct: 50,
            max_slippage_bps: 100,
            max_session_gas_wei: U256::from(10_000_000_000_000_000u64),
            max_consecutive_errors: 3,
            required_rpc_quorum: 1,
        },
    );

    assert_eq!(trader.address(), burner_addr);

    // Autonomous swap/call execution
    let target = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
    let outcome = trader
        .execute_swap(
            target,
            None,
            Bytes::new(),
            U256::from(1_000_000_000_000_000_000u64), // 1 ETH
            U256::from(1_000_000_000_000_000_000u64),
            50, // 50 bps slippage
        )
        .await
        .unwrap();

    assert!(!outcome.dry_run);
    assert!(!outcome.tx_hash.is_zero());
    assert!(!trader.circuit_breaker().is_tripped());
}

#[tokio::test]
async fn test_degen_trader_dry_run_skips_broadcast() {
    let anvil = AnvilGuard::spawn(8558);
    let rpc_url = anvil.rpc_url.clone();

    let burner_signer: PrivateKeySigner =
        "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
            .parse()
            .unwrap();

    let trader = DegenTrader::new(
        burner_signer,
        vec![rpc_url],
        31337,
        CircuitBreakerConfig {
            max_position_pct: 50,
            max_slippage_bps: 100,
            max_session_gas_wei: U256::from(10_000_000_000_000_000u64),
            max_consecutive_errors: 3,
            required_rpc_quorum: 1,
        },
    )
    .with_dry_run(true);

    assert!(trader.is_dry_run());

    let target = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
    let before = trader.circuit_breaker().is_tripped();
    let outcome = trader
        .execute_swap(
            target,
            None,
            Bytes::new(),
            U256::from(1_000_000_000_000_000_000u64),
            U256::from(1_000_000_000_000_000_000u64),
            50,
        )
        .await
        .unwrap();

    assert!(outcome.dry_run);
    assert!(outcome.tx_hash.is_zero());
    assert_eq!(before, trader.circuit_breaker().is_tripped());
}
