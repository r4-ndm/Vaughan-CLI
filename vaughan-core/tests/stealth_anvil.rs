//! Local Anvil stealth flow: plant the canonical announcer, then
//! send → scan → sweep through [`WalletState`] (the TUI backend).
//!
//! Requires `anvil` on PATH. Chain id 943 matches the built-in testnet so
//! signing hits this node.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};
use secrecy::SecretString;
use serde_json::{json, Value};
use vaughan_core::core::WalletState;
use vaughan_core::security::hd_wallet::validate_mnemonic;
use vaughan_core::security::stealth::ERC5564_ANNOUNCER;

const ANVIL_MNEMONIC: &str = "test test test test test test test test test test test junk";
/// BIP-39 test vector — not a funded anvil account; used as Bob's vault.
const BOB_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const PASSWORD: &str = "BombProof123!";
const ANNOUNCER_RUNTIME: &str = include_str!("../../scripts/erc5564/ERC5564Announcer.runtime.hex");

struct Anvil {
    child: Child,
    url: String,
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Anvil {
    fn start() -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = Command::new("anvil")
            .args(["--port", &port.to_string(), "--chain-id", "943", "--silent"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("anvil must be on PATH (foundry)");
        let anvil = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if rpc(&anvil.url, "eth_chainId", json!([])).is_ok() {
                return anvil;
            }
            if Instant::now() > deadline {
                panic!("anvil did not start");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn rpc(url: &str, method: &str, params: Value) -> Result<Value, String> {
    let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    let out = Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
        ])
        .arg(body.to_string())
        .arg(url)
        .output()
        .expect("curl");
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    if let Some(err) = v.get("error") {
        return Err(err.to_string());
    }
    Ok(v["result"].clone())
}

fn plant_announcer(url: &str) {
    let code = ANNOUNCER_RUNTIME.trim();
    rpc(
        url,
        "anvil_setCode",
        json!([format!("{ERC5564_ANNOUNCER:#x}"), code]),
    )
    .expect("anvil_setCode announcer");
    let size = rpc(
        url,
        "eth_getCode",
        json!([format!("{ERC5564_ANNOUNCER:#x}"), "latest"]),
    )
    .unwrap();
    assert!(
        size.as_str().unwrap().len() > 4,
        "announcer must have code after setCode"
    );
}

fn funded_wallet(dir: &std::path::Path, rpc_url: &str) -> WalletState {
    wallet_from_mnemonic(dir, rpc_url, ANVIL_MNEMONIC)
}

fn wallet_from_mnemonic(dir: &std::path::Path, rpc_url: &str, mnemonic: &str) -> WalletState {
    let mut wallet = WalletState::load(dir.join("wallet.json")).unwrap();
    wallet
        .create(
            &SecretString::from(PASSWORD.to_string()),
            validate_mnemonic(mnemonic).unwrap(),
        )
        .unwrap();
    wallet.set_active_network("pulsechain-testnet-v4").unwrap();
    wallet.set_rpc_override(rpc_url);
    wallet
}

fn wei_balance(url: &str, addr: Address) -> U256 {
    rpc(
        url,
        "eth_getBalance",
        json!([format!("{addr:#x}"), "latest"]),
    )
    .unwrap()
    .as_str()
    .unwrap()
    .parse()
    .unwrap()
}

fn mine(url: &str, blocks: u64) {
    rpc(url, "anvil_mine", json!([blocks])).expect("anvil_mine");
}

#[tokio::test]
async fn send_stealth_fails_without_announcer() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let uri = wallet.stealth_uri().unwrap();
    let announcement = wallet.prepare_stealth_payment(&uri).unwrap();
    let err = wallet
        .send_stealth(&announcement, &(10u128.pow(18)).to_string())
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("announcer"),
        "expected announcer-missing error, got {err}"
    );
}

#[tokio::test]
async fn send_scan_sweep_on_anvil() {
    let anvil = Anvil::start();
    plant_announcer(&anvil.url);
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);

    let uri = wallet.stealth_uri().unwrap();
    assert!(uri.starts_with("st:tpls:0x"), "{uri}");

    let announcement = wallet.prepare_stealth_payment(&uri).unwrap();
    let stealth = announcement.stealth_address;
    let amount = U256::from(10u128.pow(18)); // 1 tPLS stipend

    let sent = wallet
        .send_stealth(&announcement, &amount.to_string())
        .await
        .expect("send_stealth");
    assert_eq!(sent.stealth_address, stealth);

    let notes = wallet.scan_stealth_notes().await.expect("scan");
    assert_eq!(notes.len(), 1, "exactly one funded note");
    assert_eq!(notes[0].announcement.stealth_address, stealth);
    assert_eq!(notes[0].balance_wei, amount);

    let alice: Address = wallet.active_address().unwrap().parse().unwrap();
    let before: U256 = rpc(
        &anvil.url,
        "eth_getBalance",
        json!([format!("{alice:#x}"), "latest"]),
    )
    .unwrap()
    .as_str()
    .unwrap()
    .parse()
    .unwrap();

    wallet.sweep_stealth_note(&notes[0]).await.expect("sweep");

    let after_notes = wallet.scan_stealth_notes().await.expect("rescan");
    assert!(
        after_notes.is_empty(),
        "swept note should drop out of the funded list"
    );

    let leftover: U256 = rpc(
        &anvil.url,
        "eth_getBalance",
        json!([format!("{stealth:#x}"), "latest"]),
    )
    .unwrap()
    .as_str()
    .unwrap()
    .parse()
    .unwrap();
    assert!(
        leftover < U256::from(1_000_000_000_000_000u64),
        "stealth leftover < 0.001 tPLS, was {leftover}"
    );

    let alice_after: U256 = rpc(
        &anvil.url,
        "eth_getBalance",
        json!([format!("{alice:#x}"), "latest"]),
    )
    .unwrap()
    .as_str()
    .unwrap()
    .parse()
    .unwrap();
    assert!(alice_after > before, "alice should receive the sweep");
}

#[tokio::test]
async fn scan_stealth_notes_fails_without_announcer() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let err = wallet.scan_stealth_notes().await.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("announcer"),
        "expected announcer-missing error, got {err}"
    );
}

#[tokio::test]
async fn scan_is_empty_when_nothing_was_received() {
    let anvil = Anvil::start();
    plant_announcer(&anvil.url);
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let notes = wallet.scan_stealth_notes().await.expect("scan");
    assert!(notes.is_empty(), "fresh wallet must have no funded notes");
}

#[tokio::test]
async fn alice_pays_bob_bob_scans_and_sweeps() {
    let anvil = Anvil::start();
    plant_announcer(&anvil.url);
    let alice_dir = tempfile::tempdir().unwrap();
    let bob_dir = tempfile::tempdir().unwrap();
    let alice = funded_wallet(alice_dir.path(), &anvil.url);
    let bob = wallet_from_mnemonic(bob_dir.path(), &anvil.url, BOB_MNEMONIC);

    let bob_uri = bob.stealth_uri().unwrap();
    assert_ne!(alice.stealth_uri().unwrap(), bob_uri);

    let announcement = alice.prepare_stealth_payment(&bob_uri).unwrap();
    let stealth = announcement.stealth_address;
    let amount = U256::from(10u128.pow(18));
    alice
        .send_stealth(&announcement, &amount.to_string())
        .await
        .expect("alice send_stealth");

    let logs = rpc(
        &anvil.url,
        "eth_getLogs",
        json!([{
            "fromBlock": "0x0",
            "toBlock": "latest",
            "address": format!("{ERC5564_ANNOUNCER:#x}"),
        }]),
    )
    .unwrap();
    assert!(
        logs.as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "announcer must emit at least one log"
    );

    let bob_notes = bob.scan_stealth_notes().await.expect("bob scan");
    assert_eq!(bob_notes.len(), 1, "bob must see the note");
    assert_eq!(bob_notes[0].announcement.stealth_address, stealth);
    assert_eq!(bob_notes[0].balance_wei, amount);

    let alice_notes = alice.scan_stealth_notes().await.expect("alice scan");
    assert!(alice_notes.is_empty(), "alice must not claim bob's note");

    let bob_addr: Address = bob.active_address().unwrap().parse().unwrap();
    let before = wei_balance(&anvil.url, bob_addr);
    bob.sweep_stealth_note(&bob_notes[0])
        .await
        .expect("bob sweep");
    assert!(bob.scan_stealth_notes().await.unwrap().is_empty());
    assert!(
        wei_balance(&anvil.url, bob_addr) > before,
        "bob's public address should receive the sweep"
    );
    assert!(wei_balance(&anvil.url, stealth) < U256::from(1_000_000_000_000_000u64));
}

#[tokio::test]
async fn two_notes_sweep_one_leaves_the_other() {
    let anvil = Anvil::start();
    plant_announcer(&anvil.url);
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let uri = wallet.stealth_uri().unwrap();
    let amount = U256::from(10u128.pow(18));

    let first = wallet.prepare_stealth_payment(&uri).unwrap();
    wallet
        .send_stealth(&first, &amount.to_string())
        .await
        .expect("first send");
    let second = wallet.prepare_stealth_payment(&uri).unwrap();
    wallet
        .send_stealth(&second, &amount.to_string())
        .await
        .expect("second send");
    assert_ne!(first.stealth_address, second.stealth_address);

    let notes = wallet.scan_stealth_notes().await.expect("scan");
    assert_eq!(notes.len(), 2, "two funded notes");

    wallet
        .sweep_stealth_note(&notes[0])
        .await
        .expect("sweep first");
    let remaining = wallet.scan_stealth_notes().await.expect("rescan");
    assert_eq!(remaining.len(), 1, "one note should remain");
    assert_eq!(
        remaining[0].announcement.stealth_address,
        notes[1].announcement.stealth_address
    );
}

#[tokio::test]
async fn sweep_dust_stipend_fails() {
    let anvil = Anvil::start();
    plant_announcer(&anvil.url);
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let uri = wallet.stealth_uri().unwrap();
    let announcement = wallet.prepare_stealth_payment(&uri).unwrap();
    wallet
        .send_stealth(&announcement, "1")
        .await
        .expect("send 1 wei");

    assert!(
        wallet.scan_stealth_notes().await.unwrap().is_empty(),
        "a 1-wei stipend cannot cover sweep gas, so scan omits it"
    );

    let note = vaughan_core::core::StealthNote {
        announcement,
        balance_wei: U256::from(1u64),
        balance_formatted: "1 wei".into(),
    };
    let err = wallet.sweep_stealth_note(&note).await.unwrap_err();
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("too small") || msg.contains("stipend") || msg.contains("gas"),
        "expected dust-stipend error, got {err}"
    );
}

#[tokio::test]
async fn scan_finds_note_after_later_blocks() {
    let anvil = Anvil::start();
    plant_announcer(&anvil.url);
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let uri = wallet.stealth_uri().unwrap();
    let announcement = wallet.prepare_stealth_payment(&uri).unwrap();
    wallet
        .send_stealth(&announcement, &(10u128.pow(18)).to_string())
        .await
        .expect("send_stealth");

    mine(&anvil.url, 16);

    let notes = wallet.scan_stealth_notes().await.expect("scan after mine");
    assert_eq!(
        notes.len(),
        1,
        "note must still be found after later blocks are mined"
    );
    assert_eq!(
        notes[0].announcement.stealth_address,
        announcement.stealth_address
    );
}
