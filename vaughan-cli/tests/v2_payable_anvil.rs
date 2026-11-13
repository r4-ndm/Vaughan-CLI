//! Anvil integration tests for V2 native-in payable swap execution.
//!
//! Validates:
//! 1. Native-in V2 swap encodes `swapExactETHForTokens` (payable), sets `tx.value == amount_in`,
//!    and successfully simulates (`eth_estimateGas`), signs, and broadcasts on Anvil.
//! 2. The on-chain transaction carried `amount_in` as `msg.value` and selector `0x7ff36ab5`.
//! 3. Negative test: proving the regression is caught — sending `value > 0` with the
//!    non-payable `swapExactTokensForTokens` reverts at `eth_estimateGas`.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::primitives::{address, Address, U256};
use secrecy::SecretString;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use vaughan_agent::sentient::policy::{AgentSessionPolicy, EnforcementMode};
use vaughan_cli::signerd::{MockSwapQuoter, Signer, SignerResponse, WalletBroadcaster};
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::dex_routers::wpls_for_chain;
use vaughan_core::core::{OperatingMode, SignerIntent, SwapFamily, WalletState};
use vaughan_core::security::hd_wallet::validate_mnemonic;

const ANVIL_MNEMONIC: &str = "test test test test test test test test test test test junk";
const PASSWORD: &str = "BombProof123!";
const TEST_CHAIN_ID: u64 = 943;

// PulseX V2 SwapRouter on PulseChain testnet v4 (943)
const ROUTER_943: Address = address!("DaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7");

/// Mock router bytecode matching standard Solidity payable / non-payable dispatch:
/// - Selector 0x7ff36ab5 (`swapExactETHForTokens`): payable, accepts msg.value > 0 (STOP)
/// - Selector 0x38ed1739 (`swapExactTokensForTokens`): non-payable, reverts if CALLVALUE != 0
/// - Default: REVERT
const V2_ROUTER_MOCK_BYTECODE: &[u8] = &[
    0x60, 0x00, // 0..1: PUSH1 0x00
    0x35, // 2: CALLDATALOAD
    0x60, 0xe0, // 3..4: PUSH1 0xe0
    0x1c, // 5: SHR -> selector
    0x80, // 6: DUP1
    0x63, 0x7f, 0xf3, 0x6a, 0xb5, // 7..11: PUSH4 0x7ff36ab5
    0x14, // 12: EQ
    0x60, 0x1f, // 13..14: PUSH1 0x1f (31)
    0x57, // 15: JUMPI
    0x80, // 16: DUP1
    0x63, 0x38, 0xed, 0x17, 0x39, // 17..21: PUSH4 0x38ed1739
    0x14, // 22: EQ
    0x60, 0x21, // 23..24: PUSH1 0x21 (33)
    0x57, // 25: JUMPI
    0x60, 0x00, // 26..27: PUSH1 0x00
    0x60, 0x00, // 28..29: PUSH1 0x00
    0xfd, // 30: REVERT
    0x5b, // 31: JUMPDEST (swapExactETHForTokens)
    0x00, // 32: STOP
    0x5b, // 33: JUMPDEST (swapExactTokensForTokens)
    0x34, // 34: CALLVALUE
    0x15, // 35: ISZERO
    0x60, 0x2c, // 36..37: PUSH1 0x2c (44)
    0x57, // 38: JUMPI
    0x60, 0x00, // 39..40: PUSH1 0x00
    0x60, 0x00, // 41..42: PUSH1 0x00
    0xfd, // 43: REVERT
    0x5b, // 44: JUMPDEST
    0x00, // 45: STOP
];

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Anvil {
    child: Child,
    port: u16,
}

impl Anvil {
    fn start() -> Self {
        let port = free_port();
        let child = Command::new("anvil")
            .args(["--port", &port.to_string(), "--chain-id", "943", "--silent"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("anvil must be on PATH (foundry)");
        let anvil = Self { child, port };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if anvil.rpc("eth_chainId", json!([])).is_ok() {
                return anvil;
            }
            if Instant::now() > deadline {
                panic!("anvil failed to start in time");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let out = Command::new("curl")
            .args(["-s", "-X", "POST", "-H", "Content-Type: application/json"])
            .arg("-d")
            .arg(body.to_string())
            .arg(self.url())
            .output()
            .expect("curl must be available");
        let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
        if let Some(err) = v.get("error") {
            return Err(err.to_string());
        }
        v.get("result")
            .cloned()
            .ok_or_else(|| "missing result field".into())
    }

    fn set_code(&self, address: Address, code: &[u8]) {
        let code_hex = format!("0x{}", hex::encode(code));
        self.rpc("anvil_setCode", json!([format!("{address:#x}"), code_hex]))
            .expect("anvil_setCode must succeed");
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[tokio::test]
async fn test_native_in_v2_swap_estimates_and_broadcasts_on_anvil() {
    let anvil = Anvil::start();
    anvil.set_code(ROUTER_943, V2_ROUTER_MOCK_BYTECODE);

    let dir = tempfile::tempdir().unwrap();
    let wallet_path = dir.path().join("wallet.json");
    let mut wallet =
        WalletState::load_with_session(wallet_path, OperatingMode::SentientTrader, "sentient")
            .unwrap();
    let mnemonic = validate_mnemonic(ANVIL_MNEMONIC).unwrap();
    let password = SecretString::from(PASSWORD.to_string());
    wallet.create(&password, mnemonic).unwrap();
    wallet.unlock(&password).unwrap();
    wallet.set_active_network("pulsechain-testnet-v4").unwrap();
    wallet.set_rpc_override(anvil.url());

    let active_addr = wallet.active_address().unwrap().parse::<Address>().unwrap();
    let wpls = wpls_for_chain(TEST_CHAIN_ID).expect("wpls for 943");
    let token_out = address!("29bab93456c0e97ee931c1554c7c215480aa7766");
    let amount_in = U256::from(500_000_000_000_000_000u64); // 0.5 PLS
    let expected_out = U256::from(100_000);
    let min_amount_out = U256::from(99_000); // 1% slippage floor

    let policy = AgentSessionPolicy {
        enforcement: EnforcementMode::Enforced,
        daily_cap_pct: 100,
        max_slippage_bps: 100,
        ..Default::default()
    };
    vaughan_agent::sentient::save_policy(dir.path(), &policy).unwrap();

    let quoter = Arc::new(MockSwapQuoter::new());
    quoter.set_v2(expected_out);

    let wallet_arc = Arc::new(Mutex::new(wallet));
    let broadcaster = Arc::new(WalletBroadcaster::new(wallet_arc.clone()));

    let signer = Signer::new(
        dir.path().to_path_buf(),
        active_addr,
        TEST_CHAIN_ID,
        anvil.url(),
        quoter,
        broadcaster,
    );

    // 1. Submit native-in V2 swap intent to Signer
    let intent = SignerIntent::Swap {
        chain_id: TEST_CHAIN_ID,
        family: SwapFamily::V2,
        router: ROUTER_943,
        path: vec![wpls, token_out],
        amount_in,
        min_amount_out,
        fee: None,
    };
    let req = json!({
        "op": "sign",
        "intent": intent,
    });

    let resp_str = signer.handle_line(&req.to_string()).await;
    let resp: SignerResponse = serde_json::from_str(&resp_str).unwrap();

    assert!(
        resp.ok,
        "native-in V2 swap must sign and broadcast successfully: {:?}",
        resp.message
    );
    let tx_hash = resp.tx_hash.expect("tx_hash on success");
    assert!(
        tx_hash.starts_with("0x") && tx_hash.len() == 66,
        "expected 66-char tx hash, got {tx_hash}"
    );

    // 2. Verify mined transaction on Anvil
    let tx_obj = anvil
        .rpc("eth_getTransactionByHash", json!([tx_hash]))
        .expect("query tx by hash");
    let val_hex = tx_obj["value"]
        .as_str()
        .expect("value hex string")
        .trim_start_matches("0x");
    let onchain_value = U256::from_str_radix(val_hex, 16).expect("parse value hex");
    assert_eq!(
        onchain_value, amount_in,
        "broadcast tx on anvil must carry amount_in as value"
    );

    let input_hex = tx_obj["input"].as_str().expect("input calldata");
    assert!(
        input_hex.starts_with("0x7ff36ab5"),
        "calldata must use swapExactETHForTokens selector 0x7ff36ab5, got {input_hex}"
    );

    // 3. Negative test: prove the old failure mode
    // Sending value > 0 with swapExactTokensForTokens (0x38ed1739) MUST revert at eth_estimateGas
    let broken_tx = EvmTransaction {
        from: format!("{active_addr:#x}"),
        to: format!("{ROUTER_943:#x}"),
        value: amount_in.to_string(),
        data: Some(format!(
            "0x38ed1739{}",
            "0".repeat(64) // dummy parameters
        )),
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id: TEST_CHAIN_ID,
    };

    let est_broken = anvil.rpc(
        "eth_estimateGas",
        json!([{
            "from": format!("{active_addr:#x}"),
            "to": format!("{ROUTER_943:#x}"),
            "value": format!("{amount_in:#x}"),
            "data": broken_tx.data.as_ref().unwrap(),
        }]),
    );
    assert!(
        est_broken.is_err(),
        "direct eth_estimateGas with value on non-payable function must revert"
    );

    let w = wallet_arc.lock().await;
    let send_err = w.send_transaction(broken_tx).await.unwrap_err();
    let err_msg = send_err.to_string();
    assert!(
        err_msg.contains("revert")
            || err_msg.contains("gas estimation failed")
            || err_msg.contains("estimate the fee"),
        "non-payable swapExactTokensForTokens with value must fail at eth_estimateGas: {err_msg}"
    );
}
