//! TCP client for the TUI MCP control plane (loopback :8746).

use std::str::FromStr;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use vaughan_core::core::mcp_control_port;
use vaughan_core::core::mcp_ipc::{decode_line, encode_line, McpIpcRequest, McpIpcResponse};
use vaughan_core::core::proposal::TxProposal;

const IPC_TIMEOUT: Duration = Duration::from_secs(120);

/// Try to propose via live TUI socket. Returns `None` when TUI is offline.
pub async fn try_propose_live(
    token: &str,
    source: &str,
    proposal: &TxProposal,
) -> Result<Option<Value>, String> {
    let addr = format!("127.0.0.1:{}", mcp_control_port());
    let stream = match timeout(Duration::from_secs(2), TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("socket connect failed: {e}")),
        Err(_) => return Ok(None),
    };

    let req = McpIpcRequest::Propose {
        token: token.to_string(),
        source: source.to_string(),
        proposal: Box::new(proposal.clone()),
    };
    let line = encode_line(&req).map_err(|e| e.to_string())?;
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;

    let mut buf = BufReader::new(reader);
    let mut response_line = String::new();
    timeout(IPC_TIMEOUT, buf.read_line(&mut response_line))
        .await
        .map_err(|_| "TUI approval timed out".to_string())?
        .map_err(|e| e.to_string())?;

    let resp: McpIpcResponse =
        decode_line(&response_line).map_err(|e| format!("invalid IPC response: {e}"))?;
    if resp.ok {
        Ok(resp.data)
    } else {
        let (code, msg) = resp
            .error
            .map(|e| (e.code, e.message))
            .unwrap_or_else(|| ("ipc_error".into(), "unknown IPC error".into()));
        Err(format!("{code}: {msg}"))
    }
}

/// Query proposal status via live TUI socket.
pub async fn try_proposal_status(token: &str, proposal_id: &str) -> Result<Option<Value>, String> {
    let addr = format!("127.0.0.1:{}", mcp_control_port());
    let stream = match timeout(Duration::from_secs(2), TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("socket connect failed: {e}")),
        Err(_) => return Ok(None),
    };

    let req = McpIpcRequest::ProposalStatus {
        token: token.to_string(),
        proposal_id: proposal_id.to_string(),
    };
    let line = encode_line(&req).map_err(|e| e.to_string())?;
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;

    let mut buf = BufReader::new(reader);
    let mut response_line = String::new();
    timeout(Duration::from_secs(5), buf.read_line(&mut response_line))
        .await
        .map_err(|_| "status query timed out".to_string())?
        .map_err(|e| e.to_string())?;

    let resp: McpIpcResponse =
        decode_line(&response_line).map_err(|e| format!("invalid IPC response: {e}"))?;
    if resp.ok {
        Ok(resp.data)
    } else {
        Ok(Some(json!({
            "status": "pending_user",
            "proposal_id": proposal_id,
        })))
    }
}

/// Query the live TUI session (address + network) when unlocked.
pub async fn try_get_session(token: &str) -> Result<Option<McpSessionInfo>, String> {
    let addr = format!("127.0.0.1:{}", mcp_control_port());
    let stream = match timeout(Duration::from_secs(2), TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("socket connect failed: {e}")),
        Err(_) => return Ok(None),
    };

    let req = McpIpcRequest::Session {
        token: token.to_string(),
    };
    let line = encode_line(&req).map_err(|e| e.to_string())?;
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;

    let mut buf = BufReader::new(reader);
    let mut response_line = String::new();
    timeout(Duration::from_secs(5), buf.read_line(&mut response_line))
        .await
        .map_err(|_| "session query timed out".to_string())?
        .map_err(|e| e.to_string())?;

    let resp: McpIpcResponse =
        decode_line(&response_line).map_err(|e| format!("invalid IPC response: {e}"))?;
    if !resp.ok {
        return Ok(None);
    }
    let data = resp
        .data
        .ok_or_else(|| "session response missing data".to_string())?;
    let address = data
        .get("address")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "session response missing address".to_string())?;
    let chain_id = data
        .get("chain_id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "session response missing chain_id".to_string())?;
    let network_id = data
        .get("network_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "session response missing network_id".to_string())?
        .to_string();
    let address = alloy::primitives::Address::from_str(address)
        .map_err(|e| format!("invalid session address: {e}"))?;
    Ok(Some(McpSessionInfo {
        address,
        chain_id,
        network_id,
    }))
}

/// Active wallet session returned by the TUI listener.
#[derive(Debug, Clone)]
pub struct McpSessionInfo {
    pub address: alloy::primitives::Address,
    pub chain_id: u64,
    pub network_id: String,
}

/// Ping the TUI listener to see if it is up and token matches.
pub async fn ping(token: &str) -> bool {
    let addr = format!("127.0.0.1:{}", mcp_control_port());
    let Ok(Ok(stream)) = timeout(Duration::from_secs(1), TcpStream::connect(&addr)).await else {
        return false;
    };
    let req = McpIpcRequest::Ping {
        token: token.to_string(),
    };
    let Ok(line) = encode_line(&req) else {
        return false;
    };
    let (reader, mut writer) = stream.into_split();
    if writer.write_all(line.as_bytes()).await.is_err() {
        return false;
    }
    if writer.flush().await.is_err() {
        return false;
    }
    let mut buf = BufReader::new(reader);
    let mut response_line = String::new();
    if timeout(Duration::from_secs(2), buf.read_line(&mut response_line))
        .await
        .is_err()
    {
        return false;
    }
    decode_line::<McpIpcResponse>(&response_line)
        .map(|r| r.ok)
        .unwrap_or(false)
}
