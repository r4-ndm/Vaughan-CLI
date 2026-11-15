//! Anvil integration tests for `LiveBalanceSource`: native `eth_getBalance`,
//! ERC-20 `balanceOf` against a planted token, strict refusal of a non-contract
//! token address, and fail-closed behavior against an unreachable RPC endpoint.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};
use serde_json::{json, Value};
use vaughan_cli::signerd::{BalanceSource, LiveBalanceSource};

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

    fn first_default_account(&self) -> Address {
        let accounts = self.rpc("eth_accounts", json!([])).expect("eth_accounts");
        let first = accounts
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .expect("anvil must expose at least one default unlocked account");
        first.parse().expect("parse default account address")
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Minimal bytecode that ignores calldata and returns a fixed 32-byte value,
/// standing in for a deployed ERC-20's `balanceOf` on a bare anvil chain.
fn erc20_balance_of_stub(balance: U256) -> Vec<u8> {
    let mut code = vec![0x7f]; // PUSH32
    code.extend_from_slice(&balance.to_be_bytes::<32>());
    code.extend_from_slice(&[
        0x60, 0x00, // PUSH1 0x00
        0x52, // MSTORE
        0x60, 0x20, // PUSH1 0x20
        0x60, 0x00, // PUSH1 0x00
        0xf3, // RETURN
    ]);
    code
}

#[tokio::test]
async fn test_live_balance_source_native_matches_eth_get_balance() {
    let anvil = Anvil::start();
    let addr = anvil.first_default_account();

    let via_source = LiveBalanceSource
        .balance_of(&anvil.url(), addr, None)
        .await
        .expect("native balance_of must succeed against a live anvil node");

    let via_rpc_hex = anvil
        .rpc("eth_getBalance", json!([format!("{addr:#x}"), "latest"]))
        .expect("eth_getBalance");
    let via_rpc =
        U256::from_str_radix(via_rpc_hex.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();

    assert_eq!(via_source, via_rpc);
    assert!(!via_source.is_zero(), "anvil's default account is funded");
}

#[tokio::test]
async fn test_live_balance_source_erc20_balance_of_planted_token() {
    let anvil = Anvil::start();
    let owner = anvil.first_default_account();
    let token = Address::repeat_byte(0xAB);
    let planted_balance = U256::from(123_456_789_000_000_000_000u128);
    anvil.set_code(token, &erc20_balance_of_stub(planted_balance));

    let via_source = LiveBalanceSource
        .balance_of(&anvil.url(), owner, Some(token))
        .await
        .expect("balanceOf against a planted ERC-20 must succeed");

    assert_eq!(via_source, planted_balance);
}

#[tokio::test]
async fn test_live_balance_source_non_contract_token_is_an_error() {
    let anvil = Anvil::start();
    let owner = anvil.first_default_account();
    // Never anvil_setCode'd: this address carries no bytecode, so `eth_call` returns
    // an empty success result rather than ABI-encoded return data.
    let non_contract = Address::repeat_byte(0xCD);

    let result = LiveBalanceSource
        .balance_of(&anvil.url(), owner, Some(non_contract))
        .await;

    assert!(
        result.is_err(),
        "a non-contract token address must error, never decode to zero: {result:?}"
    );
}

/// Bytecode returning `len` bytes of 0x01 regardless of calldata: a token whose `balanceOf`
/// answer is not exactly one ABI word must be refused, not decoded from its first word.
fn returns_n_bytes_stub(len: u8) -> Vec<u8> {
    vec![
        0x60, 0x01, // PUSH1 0x01 (fill byte)
        0x60, 0x00, // PUSH1 0x00
        0x53, // MSTORE8 at offset 0 (only the first byte matters for length)
        0x60, len, // PUSH1 len
        0x60, 0x00, // PUSH1 0x00
        0xf3, // RETURN
    ]
}

#[tokio::test]
async fn test_live_balance_source_rejects_balance_of_return_that_is_not_one_word() {
    let anvil = Anvil::start();
    let owner = anvil.first_default_account();

    for (byte, len) in [(0xA1u8, 31u8), (0xA2, 33), (0xA3, 64)] {
        let token = Address::repeat_byte(byte);
        anvil.set_code(token, &returns_n_bytes_stub(len));
        let result = LiveBalanceSource
            .balance_of(&anvil.url(), owner, Some(token))
            .await;
        assert!(
            result.is_err(),
            "a {len}-byte balanceOf return must be refused, got {result:?}"
        );
    }
}

#[tokio::test]
async fn test_live_balance_source_unreachable_rpc_errs_within_timeout() {
    // A closed local port: nothing is listening, so this fails fast rather than
    // hanging the daemon on a dead RPC endpoint.
    let dead_port = free_port();
    let dead_url = format!("http://127.0.0.1:{dead_port}");

    let start = Instant::now();
    let result = LiveBalanceSource
        .balance_of(&dead_url, Address::repeat_byte(0x11), None)
        .await;
    let elapsed = start.elapsed();

    assert!(result.is_err(), "unreachable RPC must error, not hang");
    assert!(
        elapsed < Duration::from_secs(15),
        "balance_of against a dead RPC must fail within the internal timeout, took {elapsed:?}"
    );
}
