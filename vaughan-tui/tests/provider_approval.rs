//! End-to-end provider approval-flow tests against a local Anvil node.
//!
//! Drives the real stack: WebSocket client (the dApp) → `ProviderServer` →
//! `Eip1193Handler` → `ProviderHost` (the TUI's wallet handle) → approval
//! decision (simulated UI) → real vault signing + broadcast.
//!
//! Requires `anvil` on PATH. Run with:
//! ```sh
//! cargo test -p vaughan-tui --test provider_approval -- --nocapture
//! ```

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use secrecy::SecretString;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use vaughan_core::chains::evm::networks::get_network_by_chain_id;
use vaughan_core::core::WalletState;
use vaughan_core::security::hd_wallet::validate_mnemonic;
use vaughan_provider::Eip1193Handler;
use vaughan_provider::ProviderError;
use vaughan_tui::provider::{ApprovalKind, HostRequest, ProviderHost};

/// Anvil's default dev mnemonic — the wallet restored from it is funded.
const ANVIL_MNEMONIC: &str = "test test test test test test test test test test test junk";
/// Anvil dev account #0's private key (from the dev mnemonic, index 0).
const ANVIL_KEY0: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const PASSWORD: &str = "BombProof123!";

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

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
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
    fn wei_balance(&self, addr: &str) -> u128 {
        let v = self.rpc("eth_getBalance", json!([addr, "latest"])).unwrap();
        u128::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A funded wallet (restored from the anvil mnemonic) in a temp vault,
/// pointed at the local anvil RPC (chain id 943 matches the built-in
/// pulsechain-testnet-v4 network, so signing/fee-estimation hit anvil).
fn funded_wallet(dir: &Path, anvil: &Anvil) -> WalletState {
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
fn anvil_dev_address(index: u32) -> String {
    let out = Command::new("cast")
        .args(["wallet", "address", "--mnemonic", ANVIL_MNEMONIC])
        .args(["--mnemonic-index", &index.to_string()])
        .output()
        .expect("cast must be available");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Verify an EIP-191 personal-message signature with `cast wallet verify`
/// (foundry's reference implementation). Returns true if the signature
/// recovers to `address`. `decoded_message` is the UTF-8 message whose bytes
/// were signed (the provider decodes the `0x`-hex wire value before signing).
fn verify_eip191(address: &str, decoded_message: &str, signature: &str) -> bool {
    let out = Command::new("cast")
        .args(["wallet", "verify", "--address", address, decoded_message, signature])
        .output()
        .expect("cast must be available");
    out.status.success()
}

/// Spawn the full provider stack and return (server task, ws URL, request rx).
async fn spawn_provider_stack(
) -> (
    tokio::task::JoinHandle<Result<(), vaughan_provider::ProviderError>>,
    String,
    mpsc::UnboundedReceiver<HostRequest>,
) {
    let (requests, rx) = mpsc::unbounded_channel();
    let host = ProviderHost::new(requests);
    let handler = Eip1193Handler::new(Arc::new(host));
    let server = vaughan_provider::ProviderServer::bind(0).await.unwrap();
    let url = server.url();
    let events = vaughan_provider::EventBus::new();
    let task = tokio::spawn(server.serve(Arc::new(handler), events));
    (task, url, rx)
}

/// Simulated UI thread: drain approval requests, apply `decide`, reply.
async fn run_approval_consumer(
    mut rx: mpsc::UnboundedReceiver<HostRequest>,
    mut wallet: WalletState,
    decide: fn(&ApprovalKind) -> Result<(), ProviderError>,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
) {
    while let Some(request) = rx.recv().await {
        match request {
            HostRequest::Approval { kind, reply, .. } => {
                // Mirror the app: a locked wallet never prompts.
                if !wallet.is_unlocked() {
                    let _ = reply.send(Err(ProviderError::Unauthorized(
                        "wallet is locked; unlock it first".to_string(),
                    )));
                    continue;
                }
                let summary = match &*kind {
                    ApprovalKind::SendTransaction(tx) => {
                        format!("send:{}", tx.value.as_deref().unwrap_or("0"))
                    }
                    ApprovalKind::SignTransaction(_) => "sign".to_string(),
                    ApprovalKind::SignMessage { .. } => "message".to_string(),
                    ApprovalKind::SignTypedData { .. } => "typed".to_string(),
                };
                seen.lock().unwrap().push(summary.clone());
                let result = match decide(&kind) {
                    Ok(()) => vaughan_tui::provider::execute_approval(&kind, &wallet).await,
                    Err(e) => Err(e),
                };
                let _ = reply.send(result);
            }
            HostRequest::Accounts { reply } => {
                // WalletHandle contract: `[]` when locked (see methods.rs).
                let accounts = if wallet.is_unlocked() {
                    wallet
                        .active_address()
                        .map(|a| vec![a.to_string()])
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let _ = reply.send(Ok(accounts));
            }
            HostRequest::RequestAccounts { reply } => {
                let accounts = if wallet.is_unlocked() {
                    wallet
                        .active_address()
                        .map(|a| vec![a.to_string()])
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let _ = reply.send(Ok(accounts));
            }
            HostRequest::ChainId { reply } => {
                let id = wallet.networks().active().chain_id;
                let _ = reply.send(Ok(format!("0x{id:x}")));
            }
            HostRequest::SwitchChain { chain_id, reply } => {
                // Mirror the app: switch to a built-in network by chain id.
                let id: u64 = match chain_id.parse() {
                    Ok(id) => id,
                    Err(_) => {
                        let _ = reply.send(Err(ProviderError::UnrecognizedChain(chain_id)));
                        continue;
                    }
                };
                match get_network_by_chain_id(id) {
                    Some(net) => {
                        let result = wallet.set_active_network(&net.id);
                        let _ = reply.send(result.map_err(|e| {
                            ProviderError::Internal(e.user_message())
                        }));
                    }
                    None => {
                        let _ = reply.send(Err(ProviderError::UnrecognizedChain(
                            format!("0x{id:x}"),
                        )));
                    }
                }
            }
        }
    }
}



/// Send one JSON-RPC request over `ws` and return the parsed reply.
async fn rpc_call(
    ws: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    ws.send(Message::Text(
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string().into(),
    ))
    .await
    .unwrap();
    let reply = timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("reply timeout")
        .expect("socket closed")
        .unwrap();
    let Message::Text(text) = reply else {
        panic!("expected text reply");
    };
    serde_json::from_str(&text).unwrap()
}

/// The dApp sends `eth_sendTransaction`; the user approves; the tx must land
/// on anvil (recipient balance + exact value, sender nonce increments).
#[tokio::test(flavor = "multi_thread")]
async fn approve_send_broadcasts_to_anvil() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();
    let recipient = anvil_dev_address(1);
    let before = anvil.wei_balance(&recipient);
    let sender_nonce_before = anvil.rpc("eth_getTransactionCount", json!([sender, "latest"])).unwrap();

    let (task, url, rx) = spawn_provider_stack().await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let consumer = tokio::spawn(run_approval_consumer(rx, wallet, |_| Ok(()), seen2));

    let (mut ws, _) = connect_async(&url).await.unwrap();

    // Read methods first.
    let reply = rpc_call(&mut ws, 1, "eth_chainId", json!([])).await;
    assert_eq!(reply["result"], "0x3af");
    let reply = rpc_call(&mut ws, 2, "eth_accounts", json!([])).await;
    assert_eq!(reply["result"][0].as_str().unwrap().to_lowercase(), sender.to_lowercase());

    // The sign/send request — value 1 tPLS.
    let value_wei = 10u128.pow(18);
    let reply = rpc_call(
        &mut ws,
        3,
        "eth_sendTransaction",
        json!([{ "from": sender, "to": recipient, "value": format!("{value_wei:#x}") }]),
    )
    .await;
    if reply["error"].is_object() {
        panic!("eth_sendTransaction returned an error: {}", reply["error"]);
    }
    let tx_hash = reply["result"].as_str().expect("tx hash").to_string();
    assert!(tx_hash.starts_with("0x"));

    // The approval prompt was shown and approved.
    assert!(
        seen.lock().unwrap().iter().any(|s| s.starts_with("send:")),
        "approval prompt must have been shown"
    );

    // Funds moved on anvil.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if anvil.wei_balance(&recipient) == before + value_wei {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(anvil.wei_balance(&recipient), before + value_wei);
    let sender_nonce_after =
        anvil.rpc("eth_getTransactionCount", json!([sender, "latest"])).unwrap();
    let nonce_before = u64::from_str_radix(
        sender_nonce_before.as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let nonce_after = u64::from_str_radix(
        sender_nonce_after.as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    assert_eq!(nonce_after, nonce_before + 1, "sender nonce must increment");

    consumer.abort();
    task.abort();
}

/// The dApp sends `eth_sendTransaction`; the user denies; the client receives
/// EIP-1193 error 4001 and nothing lands on chain.
#[tokio::test(flavor = "multi_thread")]
async fn deny_send_returns_4001_and_broadcasts_nothing() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();
    let recipient = anvil_dev_address(2);
    let before = anvil.wei_balance(&recipient);
    let sender_nonce_before =
        anvil.rpc("eth_getTransactionCount", json!([sender, "latest"])).unwrap();

    let (task, url, rx) = spawn_provider_stack().await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let consumer = tokio::spawn(run_approval_consumer(
        rx,
        wallet,
        |_| Err(ProviderError::UserRejected),
        seen2,
    ));

    let (mut ws, _) = connect_async(&url).await.unwrap();
    let reply = rpc_call(
        &mut ws,
        1,
        "eth_sendTransaction",
        json!([{ "from": sender, "to": recipient, "value": "0x1" }]),
    )
    .await;

    // EIP-1193 user-rejected: 4001.
    assert_eq!(reply["error"]["code"], 4001);
    assert!(seen.lock().unwrap().iter().any(|s| s.starts_with("send:")));

    // Nothing moved, nonce untouched.
    assert_eq!(anvil.wei_balance(&recipient), before);
    let nonce = u64::from_str_radix(
        sender_nonce_before.as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let sender_nonce_after =
        anvil.rpc("eth_getTransactionCount", json!([sender, "latest"])).unwrap();
    let nonce_after = u64::from_str_radix(
        sender_nonce_after.as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    assert_eq!(nonce_after, nonce, "denied tx must not consume a nonce");

    consumer.abort();
    task.abort();
}

/// A locked vault: read methods still answer, but sign/send fails cleanly
/// (never prompts, never signs).
#[tokio::test(flavor = "multi_thread")]
async fn locked_wallet_reads_answer_but_signing_fails() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    wallet.lock();

    let (task, url, rx) = spawn_provider_stack().await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let consumer = tokio::spawn(run_approval_consumer(rx, wallet, |_| Ok(()), seen2));

    let (mut ws, _) = connect_async(&url).await.unwrap();

    // Reads still work (they never touch key material).
    let reply = rpc_call(&mut ws, 1, "eth_accounts", json!([])).await;
    assert!(reply["result"].is_array());

    // Send fails without broadcasting.
    let sender = anvil_dev_address(0);
    let recipient = anvil_dev_address(3);
    let recipient_before = anvil.wei_balance(&recipient);
    let reply = rpc_call(
        &mut ws,
        2,
        "eth_sendTransaction",
        json!([{ "from": sender, "to": recipient, "value": "0x1" }]),
    )
    .await;
    assert!(reply["error"].is_object(), "send must fail while locked");
    assert_eq!(
        reply["error"]["code"],
        4100,
        "locked wallet rejects with EIP-1193 unauthorized (4100)"
    );
    assert!(seen.lock().unwrap().is_empty(), "no approval prompt when locked");
    // Nothing moved.
    assert_eq!(
        anvil.wei_balance(&recipient),
        recipient_before,
        "locked send must not move funds"
    );

    consumer.abort();
    task.abort();
}

/// `personal_sign` (EIP-191): approval prompt shown, signature returns, and it
/// recovers to the active account via foundry's reference verifier.
#[tokio::test(flavor = "multi_thread")]
async fn personal_sign_recovers_to_active_account() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();

    let (task, url, rx) = spawn_provider_stack().await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let consumer = tokio::spawn(run_approval_consumer(rx, wallet, |_| Ok(()), seen2));

    let (mut ws, _) = connect_async(&url).await.unwrap();

    // Sign "hello" (hex-encoded per EIP-1193).
    let message_hex = "0x68656c6c6f";
    let reply = rpc_call(&mut ws, 1, "personal_sign", json!([message_hex, sender])).await;
    if reply["error"].is_object() {
        panic!("personal_sign returned an error: {}", reply["error"]);
    }
    let signature = reply["result"].as_str().expect("signature").to_string();
    assert!(signature.starts_with("0x"));
    assert_eq!(signature.len(), 2 + 65 * 2, "65-byte r||s||v signature");

    // Approval prompt was shown.
    assert!(
        seen.lock().unwrap().iter().any(|s| s == "message"),
        "personal_sign must show an approval prompt"
    );

    // Signature recovers to the active account: `0x68656c6c6f` decodes to
    // "hello", and that is what the provider signs (EIP-191).
    assert!(
        verify_eip191(&sender, "hello", &signature),
        "signature must verify against the active account"
    );

    consumer.abort();
    task.abort();
}

/// `eth_signTypedData_v4` (EIP-712): approval prompt shown, and the signature
/// matches foundry's reference signer for the same key + payload exactly.
#[tokio::test(flavor = "multi_thread")]
async fn typed_data_signature_matches_foundry_reference() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();

    let (task, url, rx) = spawn_provider_stack().await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let consumer = tokio::spawn(run_approval_consumer(rx, wallet, |_| Ok(()), seen2));

    let (mut ws, _) = connect_async(&url).await.unwrap();

    // A minimal EIP-712 payload. Chain id 943 matches the active network
    // (testnet v4) so the domain hash is stable and cast can reproduce it.
    let typed_data = json!({
        "types": {
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"},
            ],
            "Message": [{"name": "content", "type": "string"}],
        },
        "primaryType": "Message",
        "domain": {"name": "Vaughan Test", "version": "1", "chainId": 943},
        "message": {"content": "Hello, dApp!"},
    });

    let reply = rpc_call(
        &mut ws,
        1,
        "eth_signTypedData_v4",
        json!([sender, typed_data.clone()]),
    )
    .await;
    if reply["error"].is_object() {
        panic!("eth_signTypedData_v4 returned an error: {}", reply["error"]);
    }
    let signature = reply["result"].as_str().expect("signature").to_string();
    assert!(signature.starts_with("0x"));
    assert_eq!(signature.len(), 2 + 65 * 2, "65-byte r||s||v signature");

    // Approval prompt was shown.
    assert!(
        seen.lock().unwrap().iter().any(|s| s == "typed"),
        "eth_signTypedData_v4 must show an approval prompt"
    );

    // Foundry's reference signer (same key) must produce the identical
    // signature — exact cross-check of the EIP-712 hash + signing path.
    let out = Command::new("cast")
        .args(["wallet", "sign", "--data", "--private-key", ANVIL_KEY0])
        .arg(typed_data.to_string())
        .output()
        .expect("cast must be available");
    assert!(
        out.status.success(),
        "cast wallet sign failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reference = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        signature, reference,
        "wallet EIP-712 signature must match foundry's reference for the same key"
    );

    consumer.abort();
    task.abort();
}

/// `wallet_switchEthereumChain`: switches to a built-in network (chainId
/// reflects it), and unknown chains fail with EIP-1193 4902.
#[tokio::test(flavor = "multi_thread")]
async fn switch_chain_switches_builtin_and_rejects_unknown() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil); // starts on testnet v4 (943)

    let (task, url, rx) = spawn_provider_stack().await;
    let consumer = tokio::spawn(run_approval_consumer(
        rx,
        wallet,
        |_| Ok(()),
        Arc::new(std::sync::Mutex::new(Vec::new())),
    ));

    let (mut ws, _) = connect_async(&url).await.unwrap();

    // Switch to PulseChain mainnet (0x171 = 369).
    let reply = rpc_call(
        &mut ws,
        1,
        "wallet_switchEthereumChain",
        json!([{ "chainId": "0x171" }]),
    )
    .await;
    assert!(reply["result"].is_null(), "successful switch returns null");

    // The active chain now reports 0x171.
    let reply = rpc_call(&mut ws, 2, "eth_chainId", json!([])).await;
    assert_eq!(reply["result"], "0x171");

    // Switching back to the testnet works too.
    let reply = rpc_call(
        &mut ws,
        3,
        "wallet_switchEthereumChain",
        json!([{ "chainId": "0x3af" }]),
    )
    .await;
    assert!(reply["result"].is_null());
    let reply = rpc_call(&mut ws, 4, "eth_chainId", json!([])).await;
    assert_eq!(reply["result"], "0x3af");

    // An unknown chain (Ethereum mainnet is built-in, so use a random id).
    let reply = rpc_call(
        &mut ws,
        5,
        "wallet_switchEthereumChain",
        json!([{ "chainId": "0xdeadbeef" }]),
    )
    .await;
    assert_eq!(
        reply["error"]["code"],
        4902,
        "unknown chain rejects with EIP-1193 unrecognized chain (4902)"
    );

    consumer.abort();
    task.abort();
}
