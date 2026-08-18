//! Shared helpers for the anvil-backed integration tests.
//!
//! Each test spawns its own `anvil --chain-id 943` (matching the built-in
//! `pulsechain-testnet-v4` network, so signing and fee estimation hit the
//! local node) and builds a funded `WalletState` from anvil's dev mnemonic.
//!
//! This module is compiled into every test binary that declares `mod common;`;
//! items used by only some binaries would otherwise warn as dead code.
#![allow(dead_code)]

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ratatui::{backend::TestBackend, Frame, Terminal};
use secrecy::SecretString;
use serde_json::{json, Value};
use vaughan_core::core::WalletState;
use vaughan_core::security::hd_wallet::validate_mnemonic;

/// Anvil's default dev mnemonic — the wallet restored from it is funded.
pub const ANVIL_MNEMONIC: &str = "test test test test test test test test test test test junk";
/// Anvil dev account #0's private key (from the dev mnemonic, index 0).
pub const ANVIL_KEY0: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
pub const PASSWORD: &str = "BombProof123!";

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

pub struct Anvil {
    child: Child,
    port: u16,
}

impl Anvil {
    pub fn start() -> Self {
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
                panic!("anvil did not start in time");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
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

    /// Native balance of `addr` in wei.
    pub fn wei_balance(&self, addr: &str) -> u128 {
        let v = self.rpc("eth_getBalance", json!([addr, "latest"])).unwrap();
        u128::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    }

    /// The nonce of `addr` at the latest block.
    pub fn nonce(&self, addr: &str) -> u64 {
        let v = self.rpc("eth_getTransactionCount", json!([addr, "latest"])).unwrap();
        u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A brand-new, uninitialized wallet in a temp vault (no mnemonic yet) — for
/// onboarding flows that create the wallet themselves.
pub fn fresh_wallet(dir: &Path) -> WalletState {
    WalletState::load(dir.join("wallet.json")).unwrap()
}

/// A funded wallet (restored from the anvil mnemonic) in a temp vault,
/// pointed at the local anvil RPC (chain id 943 matches the built-in
/// pulsechain-testnet-v4 network, so signing/fee-estimation hit anvil).
pub fn funded_wallet(dir: &Path, anvil: &Anvil) -> WalletState {
    let path = dir.join("wallet.json");
    let mut wallet = WalletState::load(path).unwrap();
    let mnemonic = validate_mnemonic(ANVIL_MNEMONIC).unwrap();
    wallet
        .create(&SecretString::from(PASSWORD.to_string()), mnemonic)
        .unwrap();
    wallet.set_active_network("pulsechain-testnet-v4").unwrap();
    wallet.set_rpc_override(anvil.url());
    wallet
}

/// Anvil dev account `index` address (from the dev mnemonic).
pub fn anvil_dev_address(index: u32) -> String {
    let out = Command::new("cast")
        .args(["wallet", "address", "--mnemonic", ANVIL_MNEMONIC])
        .args(["--mnemonic-index", &index.to_string()])
        .output()
        .expect("cast must be available");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Render a view into a headless ratatui buffer and return its full text.
///
/// The closure receives a `Frame` covering the whole `width`×`height` buffer,
/// so a view's `render(frame, f.area(), …)` can be driven without a terminal.
pub fn render_frame(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(draw).unwrap();
    let buf = terminal.backend().buffer();
    let w = buf.area().width as usize;
    buf.content()
        .chunks(w)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
