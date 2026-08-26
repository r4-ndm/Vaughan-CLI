//! Freedom Browser ↔ Vaughan provider bridge smoke tests.
//!
//! Exercises the exact Origin + JSON-RPC methods Freedom's Vaughan transport
//! uses (`https://freedom.browser` → `eth_requestAccounts` / `eth_accounts` /
//! `personal_sign` / `vaughan_signTransaction` / `eth_signTypedData_v4`).
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
use vaughan_tui::provider::{HostRequest, ProviderHost};

const FREEDOM_ORIGIN: &str = "https://freedom.browser";

async fn spawn_freedom_provider_stack() -> (
    tokio::task::JoinHandle<Result<(), ProviderError>>,
    String,
    mpsc::UnboundedReceiver<HostRequest>,
) {
    let (requests, rx) = mpsc::unbounded_channel();
    let host = ProviderHost::new(requests);
    let handler = Eip1193Handler::new(Arc::new(host));
    let server = vaughan_provider::ProviderServer::bind(0)
        .await
        .unwrap()
        .with_trusted_origins([FREEDOM_ORIGIN])
        .unwrap();
    let url = server.url();
    let events = vaughan_provider::EventBus::new();
    let task = tokio::spawn(server.serve(Arc::new(handler), events));
    (task, url, rx)
}

async fn run_approval_consumer(
    mut rx: mpsc::UnboundedReceiver<HostRequest>,
    mut wallet: WalletState,
) {
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
                let result = vaughan_tui::provider::execute_approval(&kind, &wallet).await;
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
        }
    }
}

async fn connect_as_freedom(url: &str) -> WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", FREEDOM_ORIGIN.parse().unwrap());
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
async fn freedom_origin_required_when_allowlist_enabled() {
    let (task, url, _rx) = spawn_freedom_provider_stack().await;
    assert_rpc_session_dies(&url, None).await;
    assert_rpc_session_dies(&url, Some("https://evil.example")).await;
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn freedom_transport_methods_smoke_with_auto_approve() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();
    let recipient = common::anvil_dev_address(1);

    let (task, url, rx) = spawn_freedom_provider_stack().await;
    let consumer = tokio::spawn(run_approval_consumer(rx, wallet));
    let mut ws = connect_as_freedom(&url).await;

    // Freedom account discovery: eth_requestAccounts, fallback eth_accounts.
    let reply = rpc_call(&mut ws, 1, "eth_requestAccounts", json!([])).await;
    assert_eq!(
        reply["result"][0].as_str().unwrap().to_lowercase(),
        sender.to_lowercase()
    );
    let reply = rpc_call(&mut ws, 2, "eth_accounts", json!([])).await;
    assert_eq!(
        reply["result"][0].as_str().unwrap().to_lowercase(),
        sender.to_lowercase()
    );

    // personal_sign (EIP-191) — Freedom `signMessage`
    let msg = format!("0x{}", hex::encode(b"freedom-smoke"));
    let reply = rpc_call(&mut ws, 3, "personal_sign", json!([msg, sender])).await;
    assert!(
        reply["error"].is_null(),
        "personal_sign error: {}",
        reply["error"]
    );
    assert!(reply["result"].as_str().unwrap().starts_with("0x"));

    // vaughan_signTransaction — Freedom `signTransaction`
    let reply = rpc_call(
        &mut ws,
        4,
        "vaughan_signTransaction",
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
        "vaughan_signTransaction error: {}",
        reply["error"]
    );
    let raw = reply["result"].as_str().unwrap();
    assert!(raw.starts_with("0x") && raw.len() > 10);

    // eth_signTypedData_v4 — Freedom `signTypedData`
    let typed = json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "version", "type": "string" },
                { "name": "chainId", "type": "uint256" },
                { "name": "verifyingContract", "type": "address" }
            ],
            "Mail": [
                { "name": "from", "type": "string" },
                { "name": "contents", "type": "string" }
            ]
        },
        "primaryType": "Mail",
        "domain": {
            "name": "FreedomSmoke",
            "version": "1",
            "chainId": 943,
            "verifyingContract": "0x0000000000000000000000000000000000000001"
        },
        "message": { "from": "vaughan", "contents": "hello freedom" }
    });
    let reply = rpc_call(&mut ws, 5, "eth_signTypedData_v4", json!([sender, typed])).await;
    assert!(
        reply["error"].is_null(),
        "eth_signTypedData_v4 error: {}",
        reply["error"]
    );
    assert!(reply["result"].as_str().unwrap().starts_with("0x"));

    consumer.abort();
    task.abort();
}
