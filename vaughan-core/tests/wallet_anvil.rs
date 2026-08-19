//! Local Anvil coverage for [`WalletState`] money-moving paths that the TUI
//! and CLI tests only hit indirectly: native send, sequential nonces, fee
//! estimate, sign-then-broadcast, HD account #1, curated ERC-20 assets, and
//! Transfer-log token discovery.
//!
//! Requires `anvil` on PATH. Chain id 943 matches the built-in testnet so
//! signing hits this node.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

use alloy::primitives::{keccak256, Address};
use secrecy::SecretString;
use serde_json::{json, Value};
use vaughan_core::chains::evm::tokens_for_chain;
use vaughan_core::chains::{EvmTransaction, FeeDetails};
use vaughan_core::core::WalletState;
use vaughan_core::security::hd_wallet::validate_mnemonic;

const ANVIL_MNEMONIC: &str = "test test test test test test test test test test test junk";
const PASSWORD: &str = "BombProof123!";

/// Anvil default accounts (same HD path as the restored vault).
const ACCOUNT_0: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
const ACCOUNT_1: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const ACCOUNT_6: &str = "0x976EA74026E726554dB657fA54763abd0C3a0aa9";
const ACCOUNT_7: &str = "0x14dC79964da2C08b23698B3D3cc7Ca32193d9955";
const ACCOUNT_8: &str = "0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f";

/// 1_000 tokens at 18 decimals.
const THOUSAND_TOKENS: u128 = 1_000 * 10u128.pow(18);

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

fn funded_wallet(dir: &std::path::Path, rpc_url: &str) -> WalletState {
    let mut wallet = WalletState::load(dir.join("wallet.json")).unwrap();
    wallet
        .create(
            &SecretString::from(PASSWORD.to_string()),
            validate_mnemonic(ANVIL_MNEMONIC).unwrap(),
        )
        .unwrap();
    wallet.set_active_network("pulsechain-testnet-v4").unwrap();
    wallet.set_rpc_override(rpc_url);
    wallet
}

fn wei_balance(url: &str, addr: &str) -> u128 {
    let v = rpc(url, "eth_getBalance", json!([addr, "latest"])).unwrap();
    u128::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

fn nonce(url: &str, addr: &str) -> u64 {
    let v = rpc(url, "eth_getTransactionCount", json!([addr, "latest"])).unwrap();
    u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

fn plant_code(url: &str, addr: Address, runtime: &[u8]) {
    rpc(
        url,
        "anvil_setCode",
        json!([format!("{addr:#x}"), format!("0x{}", hex::encode(runtime))]),
    )
    .expect("anvil_setCode");
}

fn wait_receipt(url: &str, tx_hash: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_err = String::new();
    loop {
        match rpc(url, "eth_getTransactionReceipt", json!([tx_hash])) {
            Ok(receipt) if !receipt.is_null() => return receipt,
            Ok(_) => {}
            Err(e) => last_err = e,
        }
        if Instant::now() > deadline {
            let tx = rpc(url, "eth_getTransactionByHash", json!([tx_hash]));
            panic!("no receipt for {tx_hash}; last rpc error: {last_err}; tx={tx:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn native_tx(from: &str, to: &str, value_wei: &str) -> EvmTransaction {
    EvmTransaction {
        from: from.to_string(),
        to: to.to_string(),
        value: value_wei.to_string(),
        data: None,
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id: 943,
    }
}

/// Dispatcher that maps 4-byte selectors to static return data (same layout as
/// the browser-engine Anvil tests).
fn assemble_dispatcher(routes: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut bytecode = vec![0x60, 0x00, 0x35, 0x60, 0xe0, 0x1c];
    let dispatch_size = bytecode.len() + routes.len() * 11 + 5;
    let mut handlers = Vec::new();
    let mut current_offset = dispatch_size;

    for (_sel, ret_data) in routes {
        let handler_target = current_offset as u16;
        handlers.push((handler_target, ret_data.clone()));
        let chunks = if ret_data.is_empty() {
            0
        } else {
            ret_data.len().div_ceil(32)
        };
        current_offset += 1 + chunks * 36 + 6;
    }

    for (i, (sel, _)) in routes.iter().enumerate() {
        let (target, _) = handlers[i];
        bytecode.push(0x80); // DUP1
        bytecode.push(0x63); // PUSH4
        bytecode.extend_from_slice(sel);
        bytecode.push(0x14); // EQ
        bytecode.push(0x61); // PUSH2
        bytecode.push((target >> 8) as u8);
        bytecode.push((target & 0xff) as u8);
        bytecode.push(0x57); // JUMPI
    }
    bytecode.extend_from_slice(&[0x60, 0x00, 0x60, 0x00, 0xfd]);

    for (target, ret_data) in handlers {
        assert_eq!(bytecode.len(), target as usize);
        bytecode.push(0x5b); // JUMPDEST
        let chunks = if ret_data.is_empty() {
            0
        } else {
            ret_data.len().div_ceil(32)
        };
        for c in 0..chunks {
            let start = c * 32;
            let end = (start + 32).min(ret_data.len());
            let mut chunk_bytes = [0u8; 32];
            chunk_bytes[..end - start].copy_from_slice(&ret_data[start..end]);
            bytecode.push(0x7f); // PUSH32
            bytecode.extend_from_slice(&chunk_bytes);
            bytecode.push(0x60); // PUSH1
            bytecode.push((c * 32) as u8);
            bytecode.push(0x52); // MSTORE
        }
        let len = ret_data.len() as u16;
        bytecode.push(0x61); // PUSH2
        bytecode.push((len >> 8) as u8);
        bytecode.push((len & 0xff) as u8);
        bytecode.push(0x60); // PUSH1
        bytecode.push(0x00);
        bytecode.push(0xf3); // RETURN
    }
    bytecode
}

fn abi_encode_string(s: &str) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[31] = 0x20;
    let mut len = vec![0u8; 32];
    len[31] = s.len() as u8;
    out.extend_from_slice(&len);
    let mut data = s.as_bytes().to_vec();
    let pad = (32 - data.len() % 32) % 32;
    data.extend(std::iter::repeat_n(0, pad));
    out.extend(data);
    out
}

fn abi_encode_u256(v: u128) -> Vec<u8> {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&v.to_be_bytes());
    out.to_vec()
}

fn abi_encode_u8(v: u8) -> Vec<u8> {
    let mut out = [0u8; 32];
    out[31] = v;
    out.to_vec()
}

fn mock_erc20_runtime() -> Vec<u8> {
    assemble_dispatcher(&[
        ([0x70, 0xa0, 0x82, 0x31], abi_encode_u256(THOUSAND_TOKENS)), // balanceOf(address)
        ([0x95, 0xd8, 0x9b, 0x41], abi_encode_string("WPLS")),        // symbol()
        ([0x06, 0xfd, 0xde, 0x03], abi_encode_string("Wrapped Pulse")), // name()
        ([0x31, 0x3c, 0xe5, 0x67], abi_encode_u8(18)),                // decimals()
    ])
}

/// Runtime that emits `Transfer(msg.sender, 0x0, 1)` on any call.
///
/// LOG3 pops `offset, size, topic0, topic1, topic2` with offset on top of
/// the stack, so those values are pushed in reverse.
fn transfer_log_runtime() -> Vec<u8> {
    let topic0 = keccak256(b"Transfer(address,address,uint256)");
    let mut code = Vec::new();
    // mem[0..32] = 1 (amount)
    code.extend_from_slice(&[0x60, 0x01, 0x60, 0x00, 0x52]);
    // topic2 = 0 (to), topic1 = CALLER (from), topic0, size = 32, offset = 0
    code.extend_from_slice(&[0x60, 0x00]); // topic2
    code.push(0x33); // topic1 = CALLER
    code.push(0x7f); // topic0
    code.extend_from_slice(topic0.as_slice());
    code.extend_from_slice(&[0x60, 0x20]); // size
    code.extend_from_slice(&[0x60, 0x00]); // offset
    code.push(0xa3); // LOG3
    code.push(0x00); // STOP
    code
}

#[tokio::test]
async fn send_moves_exact_wei_and_increments_nonce() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let sender = wallet.active_address().unwrap().to_string();
    assert_eq!(sender.to_lowercase(), ACCOUNT_0.to_lowercase());

    let recipient = ACCOUNT_6;
    let before = wei_balance(&anvil.url, recipient);
    let nonce_before = nonce(&anvil.url, &sender);
    let amount = 10u128.pow(18);

    let hash = wallet
        .send(recipient, &amount.to_string())
        .await
        .expect("send");
    let receipt = wait_receipt(&anvil.url, &hash.to_string());
    assert_eq!(receipt["status"].as_str().unwrap(), "0x1");

    assert_eq!(wei_balance(&anvil.url, recipient), before + amount);
    assert_eq!(nonce(&anvil.url, &sender), nonce_before + 1);
}

#[tokio::test]
async fn sequential_sends_advance_nonce_twice() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let sender = wallet.active_address().unwrap().to_string();
    let nonce_before = nonce(&anvil.url, &sender);

    let first = wei_balance(&anvil.url, ACCOUNT_6);
    let second = wei_balance(&anvil.url, ACCOUNT_7);
    wallet
        .send(ACCOUNT_6, &(10u128.pow(18)).to_string())
        .await
        .expect("first send");
    wallet
        .send(ACCOUNT_7, &(2 * 10u128.pow(18)).to_string())
        .await
        .expect("second send");

    assert_eq!(wei_balance(&anvil.url, ACCOUNT_6), first + 10u128.pow(18));
    assert_eq!(
        wei_balance(&anvil.url, ACCOUNT_7),
        second + 2 * 10u128.pow(18)
    );
    assert_eq!(nonce(&anvil.url, &sender), nonce_before + 2);
}

#[tokio::test]
async fn estimate_fee_then_send() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let fee = wallet
        .estimate_fee(ACCOUNT_6, &(10u128.pow(18)).to_string())
        .await
        .expect("estimate_fee");
    match fee.details {
        FeeDetails::Evm { gas_limit, .. } => {
            assert!(gas_limit >= 21_000, "native transfer gas_limit={gas_limit}");
        }
        other => panic!("expected EVM fee details, got {other:?}"),
    }
    assert!(!fee.total.is_empty(), "fee total must be populated");

    let before = wei_balance(&anvil.url, ACCOUNT_6);
    wallet
        .send(ACCOUNT_6, &(10u128.pow(18)).to_string())
        .await
        .expect("send after estimate");
    assert_eq!(wei_balance(&anvil.url, ACCOUNT_6), before + 10u128.pow(18));
}

#[tokio::test]
async fn sign_transaction_then_broadcast_raw() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let sender = wallet.active_address().unwrap().to_string();
    let recipient = ACCOUNT_8;
    let before = wei_balance(&anvil.url, recipient);
    let amount = 10u128.pow(18);

    let raw = wallet
        .sign_transaction(native_tx(&sender, recipient, &amount.to_string()))
        .await
        .expect("sign_transaction");
    assert!(raw.starts_with("0x"), "signed tx must be 0x-prefixed hex");
    assert_eq!(wei_balance(&anvil.url, recipient), before);

    let bytes = hex::decode(raw.trim_start_matches("0x")).expect("signed tx hex");
    let adapter = wallet.active_adapter().await.expect("adapter");
    let hash = adapter.broadcast_raw(bytes).await.expect("broadcast_raw");
    let receipt = wait_receipt(&anvil.url, &hash.to_string());
    assert_eq!(receipt["status"].as_str().unwrap(), "0x1");
    assert_eq!(wei_balance(&anvil.url, recipient), before + amount);
}

#[tokio::test]
async fn account_one_can_send_from_its_anvil_balance() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil.url);
    wallet.set_active_account(1).expect("switch to account 1");
    let sender = wallet.active_address().unwrap().to_string();
    assert_eq!(sender.to_lowercase(), ACCOUNT_1.to_lowercase());

    let recipient = ACCOUNT_6;
    let before = wei_balance(&anvil.url, recipient);
    let nonce_before = nonce(&anvil.url, &sender);
    wallet
        .send(recipient, &(10u128.pow(18)).to_string())
        .await
        .expect("send from account 1");
    assert_eq!(wei_balance(&anvil.url, recipient), before + 10u128.pow(18));
    assert_eq!(nonce(&anvil.url, &sender), nonce_before + 1);
}

#[tokio::test]
async fn planted_wpls_shows_up_in_token_balance_and_assets() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);

    let wpls = Address::from_str(tokens_for_chain(943)[0].address).unwrap();
    plant_code(&anvil.url, wpls, &mock_erc20_runtime());

    let bal = wallet
        .token_balance(&format!("{wpls:#x}"))
        .await
        .expect("token_balance");
    assert_eq!(bal.raw, THOUSAND_TOKENS.to_string());
    assert_eq!(bal.token.symbol, "WPLS");
    assert_eq!(bal.token.decimals, 18);

    let assets = wallet.assets().await.expect("assets");
    assert!(
        assets
            .iter()
            .any(|a| a.token.contract_address.is_none()
                && a.token.symbol.eq_ignore_ascii_case("tPLS")),
        "native tPLS must be listed: {assets:?}"
    );
    let token = assets
        .iter()
        .find(|a| {
            a.token
                .contract_address
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(&format!("{wpls:#x}")))
        })
        .expect("WPLS must appear in assets");
    assert_eq!(token.raw, THOUSAND_TOKENS.to_string());
    assert_eq!(token.token.symbol, "WPLS");
}

#[tokio::test]
async fn transfer_log_is_picked_up_by_token_discovery() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil.url);
    let sender = wallet.active_address().unwrap().to_string();

    let token = Address::from_str("0x1111111111111111111111111111111111111111").unwrap();
    plant_code(&anvil.url, token, &transfer_log_runtime());

    let hash = wallet
        .send_transaction(EvmTransaction {
            from: sender,
            to: format!("{token:#x}"),
            value: "0".into(),
            data: Some("0x".into()),
            gas_limit: Some(100_000),
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: 943,
        })
        .await
        .expect("call log token");
    let _ = rpc(&anvil.url, "evm_mine", json!([]));
    let receipt = wait_receipt(&anvil.url, &hash.to_string());
    assert_eq!(
        receipt["status"].as_str().unwrap(),
        "0x1",
        "log-emitting call must succeed: {receipt}"
    );
    assert!(
        receipt["logs"]
            .as_array()
            .is_some_and(|logs| !logs.is_empty()),
        "receipt must contain Transfer logs: {receipt}"
    );

    let adapter = wallet.active_adapter().await.expect("adapter");
    let found = adapter
        .discover_token_addresses(wallet.active_address().unwrap())
        .await
        .expect("discover_token_addresses");
    assert!(
        found.contains(&token),
        "discovery must include the log-emitting token, got {found:?}"
    );
}
