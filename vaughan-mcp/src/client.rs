//! TCP client for the TUI MCP control plane (loopback :8746).

use std::str::FromStr;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use vaughan_core::core::mcp_control_port;
use vaughan_core::core::mcp_host::read_ipc_line;
use vaughan_core::core::mcp_ipc::{decode_ipc_line, encode_line, McpIpcRequest, McpIpcResponse};
use vaughan_core::core::proposal::TxProposal;

const IPC_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Try to propose via live TUI socket. Returns `None` when TUI is offline.
pub async fn try_propose_live(
    token: &str,
    source: &str,
    proposal: &TxProposal,
) -> Result<Option<Value>, String> {
    let req = McpIpcRequest::Propose {
        token: token.to_string(),
        source: source.to_string(),
        proposal: Box::new(proposal.clone()),
    };
    ipc_request(req, IPC_TIMEOUT).await
}

/// Query proposal status via live TUI socket.
pub async fn try_proposal_status(token: &str, proposal_id: &str) -> Result<Option<Value>, String> {
    let req = McpIpcRequest::ProposalStatus {
        token: token.to_string(),
        proposal_id: proposal_id.to_string(),
    };
    ipc_request(req, Duration::from_secs(5)).await
}

/// Query the live TUI session (address + network) when unlocked.
pub async fn try_get_session(token: &str) -> Result<Option<McpSessionInfo>, String> {
    let req = McpIpcRequest::Session {
        token: token.to_string(),
    };
    let data = ipc_request(req, Duration::from_secs(5)).await?;
    let Some(data) = data else {
        return Ok(None);
    };
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
    let req = McpIpcRequest::Ping {
        token: token.to_string(),
    };
    matches!(ipc_request(req, Duration::from_secs(2)).await, Ok(Some(_)))
}

/// Query stealth URI via live control plane.
pub async fn try_stealth_uri(token: &str) -> Result<Option<Value>, String> {
    ipc_request(
        McpIpcRequest::StealthUri {
            token: token.to_string(),
        },
        IPC_TIMEOUT,
    )
    .await
}

/// Scan stealth notes via live control plane.
pub async fn try_stealth_scan(token: &str) -> Result<Option<Value>, String> {
    ipc_request(
        McpIpcRequest::StealthScan {
            token: token.to_string(),
        },
        IPC_TIMEOUT,
    )
    .await
}

/// Sweep a stealth note via live control plane.
pub async fn try_stealth_sweep(
    token: &str,
    stealth_address: &str,
) -> Result<Option<Value>, String> {
    ipc_request(
        McpIpcRequest::StealthSweep {
            token: token.to_string(),
            stealth_address: stealth_address.to_string(),
        },
        IPC_TIMEOUT,
    )
    .await
}

async fn ipc_request(req: McpIpcRequest, wait: Duration) -> Result<Option<Value>, String> {
    let addr = format!("127.0.0.1:{}", mcp_control_port());
    let stream = match timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("socket connect failed: {e}")),
        Err(_) => return Ok(None),
    };

    let line = encode_line(&req).map_err(|e| e.to_string())?;
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;

    let mut buf = BufReader::new(reader);
    let response_line = timeout(wait, read_ipc_line(&mut buf))
        .await
        .map_err(|_| "IPC timed out".to_string())?
        .map_err(|e| e.to_string())?;

    let resp: McpIpcResponse =
        decode_ipc_line(&response_line).map_err(|e| format!("invalid IPC response: {e}"))?;
    if resp.ok {
        Ok(resp.data)
    } else {
        let (code, msg) = resp
            .error
            .map(|e| (e.code, e.message))
            .unwrap_or_else(|| ("ipc_error".into(), "unknown IPC error".into()));
        if code == "tui_offline" || code == "wallet_locked" {
            Ok(None)
        } else {
            Err(format!("{code}: {msg}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_request_encodes() {
        let line = encode_line(&McpIpcRequest::Ping {
            token: "abc".into(),
        })
        .unwrap();
        assert!(line.contains("\"method\":\"ping\"") || line.contains("ping"));
    }
}
