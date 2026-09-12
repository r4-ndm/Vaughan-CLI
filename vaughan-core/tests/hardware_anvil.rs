//! Anvil coverage for hardware account routing via [`MockSignerBackend`].
//!
//! No USB — proves prepare_sign_raw → mock sign → broadcast_raw for Ledger and
//! Trezor watch records (vendor-tagged vault entries; same mock signer). Vault
//! uses the abandon mnemonic so Anvil key0 does not collide with HD accounts.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use alloy::signers::local::PrivateKeySigner;
use secrecy::SecretString;
use serde_json::{json, Value};
use tempfile::TempDir;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::WalletState;
use vaughan_core::security::hd_wallet::validate_mnemonic;
use vaughan_core::security::{
    DeviceSession, HardwareVendor, HwChainFamily, MockDeviceSession, MockSignerBackend,
};

const VAULT_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const PASSWORD: &str = "BombProof123!";
const ANVIL_KEY0: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const ANVIL_ADDR0: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
const ANVIL_ADDR1: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

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
        while Instant::now() < deadline {
            if rpc(&anvil.url, "eth_chainId", json!([])).is_ok() {
                return anvil;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("anvil did not become ready");
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

/// Keep [`TempDir`] alive for the life of the returned wallet.
struct HwWallet {
    _dir: TempDir,
    wallet: WalletState,
}

/// Unlock vault with a mock HW watch for `vendor` (optional Anvil RPC override).
fn wallet_with_mock_hw(anvil_url: Option<&str>, vendor: HardwareVendor, label: &str) -> HwWallet {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = WalletState::load(dir.path().join("wallet.json")).unwrap();
    wallet
        .create(
            &SecretString::from(PASSWORD.to_string()),
            validate_mnemonic(VAULT_MNEMONIC).unwrap(),
        )
        .unwrap();
    wallet.set_active_network("pulsechain-testnet-v4").unwrap();
    if let Some(url) = anvil_url {
        wallet.set_rpc_override(url);
    }

    let mock = MockSignerBackend::new(ANVIL_KEY0.parse::<PrivateKeySigner>().unwrap());
    assert!(mock.address_string().eq_ignore_ascii_case(ANVIL_ADDR0));
    let record = mock.watch_record_for(vendor, "m/44'/60'/0'/0/0", Some("943".into()), label);
    let account = wallet.add_hardware_account(record).unwrap();
    assert!(account.kind.is_hardware());
    wallet.set_hardware_mock(mock);
    wallet.set_active_account(account.index).unwrap();
    HwWallet { _dir: dir, wallet }
}

fn sample_tx() -> EvmTransaction {
    EvmTransaction {
        from: ANVIL_ADDR0.into(),
        to: ANVIL_ADDR1.into(),
        value: "1000000000000000".into(),
        data: None,
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id: 943,
    }
}

#[tokio::test]
async fn mock_ledger_account_can_send_on_anvil() {
    let anvil = Anvil::start();
    let hw = wallet_with_mock_hw(Some(&anvil.url), HardwareVendor::Ledger, "Mock Ledger");

    let receipt = hw
        .wallet
        .broadcast(sample_tx(), "hw-mock-ledger")
        .await
        .expect("broadcast");
    assert!(receipt.hash.starts_with("0x"));

    let pw = SecretString::from(PASSWORD.to_string());
    assert!(hw.wallet.export_active_private_key(&pw).is_err());
}

#[tokio::test]
async fn mock_trezor_account_can_send_on_anvil() {
    let anvil = Anvil::start();
    let hw = wallet_with_mock_hw(Some(&anvil.url), HardwareVendor::Trezor, "Trezor 1");

    let label = hw.wallet.active_account_label().expect("label");
    assert_eq!(label, "Trezor 1");

    let receipt = hw
        .wallet
        .broadcast(sample_tx(), "hw-mock-trezor")
        .await
        .expect("broadcast");
    assert!(receipt.hash.starts_with("0x"));

    let pw = SecretString::from(PASSWORD.to_string());
    assert!(hw.wallet.export_active_private_key(&pw).is_err());
}

#[tokio::test]
async fn mock_trezor_account_can_personal_sign() {
    let anvil = Anvil::start();
    let hw = wallet_with_mock_hw(Some(&anvil.url), HardwareVendor::Trezor, "Trezor 1");

    let sig = hw
        .wallet
        .sign_message_async(b"vaughan-trezor-mock")
        .await
        .expect("personal_sign");
    assert!(
        sig.starts_with("0x") && sig.len() >= 130,
        "expected hex signature, got len {}",
        sig.len()
    );
}

#[test]
fn mock_trezor_empty_label_defaults_to_trezor_n() {
    // Empty label → AccountManager assigns "Trezor 1" (no chain I/O needed).
    let hw = wallet_with_mock_hw(None, HardwareVendor::Trezor, "");
    assert_eq!(hw.wallet.active_account_label().unwrap(), "Trezor 1");
}

#[tokio::test]
async fn mock_device_session_lists_path() {
    let session = MockDeviceSession::new("m/44'/60'/0'/0/0", ANVIL_ADDR0);
    let paths = session
        .list_paths_preview(HwChainFamily::Evm)
        .await
        .unwrap();
    assert_eq!(paths.len(), 1);
}
