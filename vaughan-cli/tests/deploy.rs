//! End-to-end tests against a local Anvil node.
//!
//! Requires `anvil` (Foundry) on PATH. Each test spawns its own anvil with the
//! default dev mnemonic on chain id 943 (matches the built-in
//! `pulsechain-testnet-v4` network, so the CLI's `--rpc-url` override works
//! with no chain-id mismatch).
//!
//! Run with:
//! ```sh
//! cargo test -p vaughan-cli --test deploy -- --nocapture
//! ```

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Anvil's default dev mnemonic — the wallet restored from it is the funded
/// account, so no faucet step is needed.
const ANVIL_MNEMONIC: &str =
    "test test test test test test test test test test test junk";
/// Anvil dev account #0.
const ANVIL_KEY0: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

const PASSWORD_ENV: &str = "VAUGHAN_TEST_PW";
const PASSWORD: &str = "BombProof123!";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_vaughan")
}

/// Find a free localhost port.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Anvil {
    child: Child,
    port: u16,
}

impl Anvil {
    fn start() -> Self {
        let port = free_port();
        let child = Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                "943",
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("anvil must be on PATH (foundry)");
        let anvil = Self { child, port };
        // Wait for the RPC to come up.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if anvil.rpc("eth_chainId", json!([])).is_ok() {
                return anvil;
            }
            if Instant::now() > deadline {
                panic!("anvil did not start in time");
            }
            std::thread::sleep(Duration::from_millis(100));
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
        Ok(v["result"].clone())
    }

    /// Code at `addr`, or empty string when none.
    fn code_at(&self, addr: &str) -> String {
        self.rpc("eth_getCode", json!([addr, "latest"]))
            .unwrap_or(Value::String(String::new()))
            .as_str()
            .unwrap_or("")
            .to_string()
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A fresh vault in a temp dir, restored from the anvil mnemonic (funded).
fn new_vault(dir: &PathBuf) -> PathBuf {
    let vault = dir.join("wallet.json");
    let out = Command::new(bin())
        .args(["--vault"])
        .arg(&vault)
        .args(["restore", ANVIL_MNEMONIC, "--password-env", PASSWORD_ENV])
        .env(PASSWORD_ENV, PASSWORD)
        .output()
        .expect("vaughan restore");
    assert!(
        out.status.success(),
        "restore failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    vault
}

/// `vaughan send`; returns (status, stdout, stderr).
fn cli_send(
    vault: &PathBuf,
    anvil: &Anvil,
    args: &[&str],
) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["--vault"])
        .arg(vault)
        .args(["send", "--network", "pulsechain-testnet-v4", "--rpc-url", &anvil.url()])
        .args(args)
        .args(["--password-env", PASSWORD_ENV])
        .env(PASSWORD_ENV, PASSWORD)
        .output()
        .expect("vaughan send");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn deploy_contract_and_verify_code() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let vault = new_vault(&dir.path().to_path_buf());

    // Minimal contract: runtime is `PUSH1 0x2a PUSH1 0x00 MSTORE PUSH1 0x20
    // PUSH1 0x00 RETURN` (returns 0x2a). Full creation bytecode: copy the
    // 10-byte runtime (offset 0x0c) to memory and return it.
    let runtime = "602a60005260206000f3";
    let bytecode = format!("0x600a600c600039600a6000f3{runtime}");
    let (ok, stdout, stderr) = cli_send(&vault, &anvil, &["--data", &bytecode, "--value", "0"]);
    assert!(ok, "deploy failed: {stderr}");
    let stdout = stdout.trim().to_string();
    assert!(stdout.starts_with("0x"), "expected a tx hash, got: {stdout}");

    // The created address comes from the tx receipt (the sender's nonce may
    // already be non-zero, so deriving it is unreliable). Poll until the
    // receipt carries a contractAddress.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut deployed = None;
    while Instant::now() < deadline {
        if let Some(addr) = receipt_contract_address(&anvil, &stdout) {
            deployed = Some(addr);
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let deployed = deployed.expect("deploy receipt must carry a contractAddress");
    assert_eq!(anvil.code_at(&deployed), format!("0x{runtime}"));
}

#[test]
fn native_transfer_moves_balance() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let vault = new_vault(&dir.path().to_path_buf());

    // Second anvil dev account (derived from the mnemonic, index 1).
    let recipient = anvil_dev_address(1);

    let before: u128 = wei_balance(&anvil, &recipient);
    let amount_wei = 10u128.pow(18); // 1 tPLS

    let (ok, stdout, stderr) =
        cli_send(&vault, &anvil, &[&recipient, "--value", &amount_wei.to_string()]);
    assert!(ok, "transfer failed: {stderr}");
    assert!(stdout.trim().starts_with("0x"), "expected tx hash: {stdout}");

    let after: u128 = wei_balance(&anvil, &recipient);
    assert_eq!(after - before, amount_wei, "recipient must receive the value");
}

#[test]
fn balance_reports_funded_account() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let vault = new_vault(&dir.path().to_path_buf());

    let out = Command::new(bin())
        .args(["--vault"])
        .arg(&vault)
        .args(["balance", "--network", "pulsechain-testnet-v4", "--rpc-url", &anvil.url()])
        .args(["--password-env", PASSWORD_ENV])
        .env(PASSWORD_ENV, PASSWORD)
        .output()
        .expect("vaughan balance");
    assert!(out.status.success(), "balance failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Anvil funds every dev account with 10000 ETH.
    assert!(stdout.contains("tPLS"), "balance output: {stdout}");
}

#[test]
fn insufficient_funds_fails_cleanly() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();

    // A fresh wallet restored from a DIFFERENT mnemonic — zero balance.
    let vault = dir.path().join("poor.json");
    let out = Command::new(bin())
        .args(["--vault"])
        .arg(&vault)
        .args([
            "restore",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "--password-env",
            PASSWORD_ENV,
        ])
        .env(PASSWORD_ENV, PASSWORD)
        .output()
        .unwrap();
    assert!(out.status.success());

    let recipient = anvil_dev_address(1);
    let (ok, _stdout, stderr) =
        cli_send(&vault, &anvil, &[&recipient, "--value", "1000000000000000000"]);
    assert!(!ok, "zero-balance send must fail");
    assert!(
        stderr.to_lowercase().contains("insufficient") || stderr.to_lowercase().contains("funds"),
        "error should mention funds: {stderr}"
    );
}

#[test]
fn wrong_password_fails_cleanly() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let vault = new_vault(&dir.path().to_path_buf());

    let out = Command::new(bin())
        .args(["--vault"])
        .arg(&vault)
        .args(["balance", "--network", "pulsechain-testnet-v4", "--rpc-url", &anvil.url()])
        .args(["--password-env", PASSWORD_ENV])
        .env(PASSWORD_ENV, "WrongPassword")
        .output()
        .unwrap();
    assert!(!out.status.success(), "wrong password must fail");
    assert!(
        String::from_utf8_lossy(&out.stderr).to_lowercase().contains("password")
            || String::from_utf8_lossy(&out.stderr).to_lowercase().contains("decrypt"),
        "error should mention password/decryption"
    );
}

// ---- helpers ----

// The vault restored from the anvil mnemonic derives the same addresses
// (both m/44'/60'/0'/0/i), so the vault's active account IS anvil dev
// account 0 — funded with 10000 ETH by default.

/// Anvil dev account `index` address (from the dev mnemonic).
fn anvil_dev_address(index: u32) -> String {
    let out = Command::new("cast")
        .args(["wallet", "address", "--mnemonic", ANVIL_MNEMONIC])
        .args(["--mnemonic-index", &index.to_string()])
        .output()
        .expect("cast must be available");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Native balance of `addr` on the anvil node (wei).
fn wei_balance(anvil: &Anvil, addr: &str) -> u128 {
    let v = anvil.rpc("eth_getBalance", json!([addr, "latest"])).unwrap();
    u128::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

/// `contractAddress` from the receipt of `tx_hash` (None until mined).
fn receipt_contract_address(anvil: &Anvil, tx_hash: &str) -> Option<String> {
    let receipt = anvil.rpc("eth_getTransactionReceipt", json!([tx_hash])).ok()?;
    if receipt.is_null() {
        return None;
    }
    receipt["contractAddress"].as_str().map(|s| s.to_string())
}
