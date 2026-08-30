//! Vaughan dApp browser ↔ provider bridge smoke (Anvil).
//!
//! Production inject uses an extension **background** WebSocket whose handshake
//! Origin is [`vaughan_tui::provider::DAPP_BROWSER_PROVIDER_ORIGIN`] (not the
//! page host). Page-origin cases below still cover Freedom-style / direct
//! inject clients and multi-host allowlisting.
//!
//! Requires `anvil` on PATH.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{funded_wallet, Anvil};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use vaughan_core::chains::evm::networks::get_network_by_chain_id;
use vaughan_core::core::WalletState;
use vaughan_provider::Eip1193Handler;
use vaughan_provider::ProviderError;
use vaughan_tui::provider::{
    HostRequest, ProviderHost, DAPP_BROWSER_PROVIDER_ORIGIN, FREEDOM_PROVIDER_ORIGIN,
};

/// Pulse-first allowlisted dApp origin (legacy page-Origin inject path).
const PULSE_DAPP_ORIGIN: &str = "https://app.pulsex.com";
/// Non-Pulse EVM dApp origin (multi-chain allowlist smoke).
const UNI_DAPP_ORIGIN: &str = "https://app.uniswap.org";

async fn spawn_dapp_provider_stack(
    origins: &[&str],
) -> (
    tokio::task::JoinHandle<Result<(), ProviderError>>,
    String,
    mpsc::Receiver<HostRequest>,
) {
    let (requests, rx) = mpsc::channel(16);
    let host = ProviderHost::new(requests);
    let handler = Eip1193Handler::new(Arc::new(host));
    let server = vaughan_provider::ProviderServer::bind(0)
        .await
        .unwrap()
        .with_trusted_origins(origins.iter().copied())
        .unwrap();
    let url = server.url();
    let events = vaughan_provider::EventBus::new();
    let task = tokio::spawn(server.serve(Arc::new(handler), events));
    (task, url, rx)
}

async fn run_approval_consumer(mut rx: mpsc::Receiver<HostRequest>, mut wallet: WalletState) {
    let mut connected = std::collections::HashSet::<String>::new();
    while let Some(request) = rx.recv().await {
        match request {
            HostRequest::Approval { kind, reply, .. } => {
                if !wallet.is_unlocked() {
                    let _ = reply.send(Err(ProviderError::Unauthorized(
                        "wallet is locked; unlock it first".to_string(),
                    )));
                    continue;
                }
                let result = vaughan_tui::provider::execute_approval(&kind, &mut wallet).await;
                let _ = reply.send(result);
            }
            HostRequest::Accounts { site, reply } => {
                let accounts = if wallet.is_unlocked() && connected.contains(&site) {
                    wallet
                        .active_address()
                        .map(|a| vec![a.to_string()])
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let _ = reply.send(Ok(accounts));
            }
            HostRequest::RequestAccounts { site, reply } => {
                let accounts = if wallet.is_unlocked() {
                    connected.insert(site);
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
            HostRequest::SwitchChain {
                chain_id, reply, ..
            } => {
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
                        let _ = reply
                            .send(result.map_err(|e| ProviderError::Internal(e.user_message())));
                    }
                    None => {
                        let _ =
                            reply.send(Err(ProviderError::UnrecognizedChain(format!("0x{id:x}"))));
                    }
                }
            }
            HostRequest::RpcRead {
                method,
                params,
                reply,
            } => {
                let snap = match wallet.network_rpc_snapshot() {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = reply.send(Err(ProviderError::Internal(e.user_message())));
                        continue;
                    }
                };
                let result = snap
                    .forward_read(&method, params)
                    .await
                    .map_err(|e| ProviderError::Internal(e.user_message()));
                let _ = reply.send(result);
            }
        }
    }
}

async fn connect_as_dapp(
    url: &str,
    origin: &str,
) -> WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", origin.parse().unwrap());
    let (ws, _) = connect_async(request).await.unwrap();
    ws
}

async fn rpc_call(
    ws: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    ws.send(Message::Text(
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
            .to_string()
            .into(),
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

async fn assert_rpc_session_dies(url: &str, origin: Option<&str>) {
    let connect = match origin {
        Some(origin) => {
            let mut request = url.into_client_request().unwrap();
            request
                .headers_mut()
                .insert("Origin", origin.parse().unwrap());
            connect_async(request).await
        }
        None => connect_async(url).await,
    };
    let (mut ws, _) = connect.expect("handshake may still complete before origin gate");
    let _ = ws
        .send(Message::Text(
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}"#.into(),
        ))
        .await;
    let next = timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("server must close untrusted session promptly")
        .expect("socket closes");
    assert!(
        matches!(next, Ok(Message::Close(_)) | Err(_)),
        "expected close/err, got {next:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dapp_page_origin_required_when_allowlist_enabled() {
    let (task, url, _rx) = spawn_dapp_provider_stack(&[PULSE_DAPP_ORIGIN]).await;
    assert_rpc_session_dies(&url, None).await;
    assert_rpc_session_dies(&url, Some("https://evil.example")).await;
    // Uniswap origin must not work when only PulseX is allowlisted.
    assert_rpc_session_dies(&url, Some(UNI_DAPP_ORIGIN)).await;
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn pulse_dapp_inject_connect_and_send_on_anvil() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();
    let recipient = common::anvil_dev_address(1);
    let before = anvil.wei_balance(&recipient);

    let (task, url, rx) = spawn_dapp_provider_stack(&[PULSE_DAPP_ORIGIN]).await;
    let consumer = tokio::spawn(run_approval_consumer(rx, wallet));
    let mut ws = connect_as_dapp(&url, PULSE_DAPP_ORIGIN).await;

    let reply = rpc_call(&mut ws, 1, "eth_chainId", json!([])).await;
    assert_eq!(reply["result"], "0x3af"); // 943

    let reply = rpc_call(&mut ws, 2, "eth_requestAccounts", json!([])).await;
    assert_eq!(
        reply["result"][0].as_str().unwrap().to_lowercase(),
        sender.to_lowercase()
    );

    let reply = rpc_call(
        &mut ws,
        3,
        "eth_sendTransaction",
        json!([{
            "from": sender,
            "to": recipient,
            "value": "0x1",
            "chainId": "0x3af"
        }]),
    )
    .await;
    assert!(
        reply["error"].is_null(),
        "eth_sendTransaction error: {}",
        reply["error"]
    );
    let tx_hash = reply["result"].as_str().unwrap();
    assert!(tx_hash.starts_with("0x") && tx_hash.len() == 66);

    let after = anvil.wei_balance(&recipient);
    assert_eq!(after, before + 1, "recipient should receive 1 wei");

    consumer.abort();
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn uniswap_origin_switch_chain_multi_evm() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);

    let (task, url, rx) = spawn_dapp_provider_stack(&[PULSE_DAPP_ORIGIN, UNI_DAPP_ORIGIN]).await;
    let consumer = tokio::spawn(run_approval_consumer(rx, wallet));
    let mut ws = connect_as_dapp(&url, UNI_DAPP_ORIGIN).await;

    let reply = rpc_call(&mut ws, 1, "eth_chainId", json!([])).await;
    assert_eq!(reply["result"], "0x3af");

    // Switch to Ethereum mainnet id (builtin) — no broadcast, just network state.
    let reply = rpc_call(
        &mut ws,
        2,
        "wallet_switchEthereumChain",
        json!([{ "chainId": "0x1" }]),
    )
    .await;
    assert!(
        reply["error"].is_null(),
        "switch chain error: {}",
        reply["error"]
    );

    let reply = rpc_call(&mut ws, 3, "eth_chainId", json!([])).await;
    assert_eq!(reply["result"], "0x1");

    consumer.abort();
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn dapp_browser_extension_origin_connects() {
    // Production Chromium shell: background SW Origin is chrome-extension://…
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();

    let (task, url, rx) =
        spawn_dapp_provider_stack(&[DAPP_BROWSER_PROVIDER_ORIGIN, FREEDOM_PROVIDER_ORIGIN]).await;
    let consumer = tokio::spawn(run_approval_consumer(rx, wallet));
    let mut ws = connect_as_dapp(&url, DAPP_BROWSER_PROVIDER_ORIGIN).await;

    let reply = rpc_call(&mut ws, 1, "eth_chainId", json!([])).await;
    assert_eq!(reply["result"], "0x3af");

    let reply = rpc_call(&mut ws, 2, "eth_requestAccounts", json!([])).await;
    assert_eq!(
        reply["result"][0].as_str().unwrap().to_lowercase(),
        sender.to_lowercase()
    );

    // Page origins alone are rejected when only extension+Freedom are trusted.
    assert_rpc_session_dies(&url, Some(PULSE_DAPP_ORIGIN)).await;

    consumer.abort();
    task.abort();
}
