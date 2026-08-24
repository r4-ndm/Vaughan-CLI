//! Loopback MCP listener roundtrip (ping + session snapshot).

use std::net::TcpListener;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;
use vaughan_core::core::mcp_ipc::{decode_line, encode_line, McpIpcRequest, McpIpcResponse};
use vaughan_core::core::mcp_control_port;
use vaughan_tui::mcp::{McpService, McpSessionSnapshot};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn ipc_roundtrip(req: McpIpcRequest) -> McpIpcResponse {
    let addr = format!("127.0.0.1:{}", mcp_control_port());
    let stream = timeout(Duration::from_secs(2), TcpStream::connect(&addr))
        .await
        .expect("connect timeout")
        .expect("connect failed");
    let line = encode_line(&req).expect("encode");
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(line.as_bytes())
        .await
        .expect("write");
    writer.flush().await.expect("flush");
    let mut buf = BufReader::new(reader);
    let mut response_line = String::new();
    timeout(Duration::from_secs(2), buf.read_line(&mut response_line))
        .await
        .expect("read timeout")
        .expect("read");
    decode_line(&response_line).expect("decode")
}

#[tokio::test]
async fn mcp_loopback_ping_and_session() {
    let port = free_port();
    std::env::set_var("VAUGHAN_MCP_PORT", port.to_string());

    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut svc = McpService::new(dir.path(), tx);
    svc.update_session(McpSessionSnapshot {
        address: Some("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".into()),
        chain_id: Some(31_337),
        network_id: Some("anvil".into()),
    });
    svc.on_unlock(&tokio::runtime::Handle::current());
    tokio::time::sleep(Duration::from_millis(200)).await;

    let token = svc.session_secret().expect("session token").to_string();

    let ping = ipc_roundtrip(McpIpcRequest::Ping {
        token: token.clone(),
    })
    .await;
    assert!(ping.ok, "ping failed: {:?}", ping.error);

    let session = ipc_roundtrip(McpIpcRequest::Session { token }).await;
    assert!(session.ok, "session failed: {:?}", session.error);
    let data = session.data.expect("session data");
    assert_eq!(
        data["address"].as_str().unwrap(),
        "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
    );
    assert_eq!(data["chain_id"].as_u64().unwrap(), 31_337);
    assert_eq!(data["network_id"].as_str().unwrap(), "anvil");

    svc.on_lock();
}
