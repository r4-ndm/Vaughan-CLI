//! Shared MCP loopback control-plane dispatch for TUI and `vaughan serve`.
//!
//! Host-specific signing/approval lives behind [`McpHostBackend`]; token checks,
//! proposal status, and wire framing are centralized here.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::core::mcp_ipc::{
    decode_ipc_line, encode_line, session_token_valid, McpIpcLineError, McpIpcRequest,
    McpIpcResponse, MCP_IPC_MAX_LINE_BYTES,
};
use crate::core::proposal::{
    proposal_status_json, validate_proposal_id, ProposalQueue, TxProposal,
};

/// Active wallet session returned to MCP clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSessionData {
    pub address: String,
    pub chain_id: u64,
    pub network_id: String,
}

/// Outcome of a live `propose` IPC call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpProposeOutcome {
    Approved {
        tx_hash: String,
    },
    Queued {
        proposal_id: String,
        message: String,
    },
    Rejected {
        reason: String,
    },
}

/// Host-specific MCP operations (TUI approval card vs serve auto-exec).
#[async_trait]
pub trait McpHostBackend: Send + Sync {
    /// Optional tag merged into JSON (`serve`, `tui`, …).
    fn host_tag(&self) -> Option<&'static str> {
        None
    }

    async fn session(&self) -> Result<McpSessionData, String>;
    async fn propose(
        &self,
        source: &str,
        proposal: TxProposal,
    ) -> Result<McpProposeOutcome, String>;
    async fn stealth_uri(&self) -> Result<String, String>;
    async fn stealth_scan(&self) -> Result<Value, String>;
    async fn stealth_sweep(&self, stealth_address: &str) -> Result<String, String>;
}

/// Read one newline-delimited IPC request with a byte cap ( enforced while reading ).
pub async fn read_ipc_line<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
) -> Result<String, McpIpcLineError> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1];
    loop {
        let n = reader
            .read(&mut chunk)
            .await
            .map_err(|e| McpIpcLineError::Parse(format!("read failed: {e}")))?;
        if n == 0 {
            if bytes.is_empty() {
                return Err(McpIpcLineError::Parse("empty IPC line".into()));
            }
            break;
        }
        if chunk[0] == b'\n' {
            break;
        }
        if bytes.len() >= MCP_IPC_MAX_LINE_BYTES {
            return Err(McpIpcLineError::TooLarge);
        }
        bytes.push(chunk[0]);
    }
    String::from_utf8(bytes).map_err(|e| McpIpcLineError::Parse(format!("invalid utf-8: {e}")))
}

/// Dispatch one authenticated IPC request.
pub async fn dispatch_ipc_request(
    req: McpIpcRequest,
    session_token: &str,
    profile_dir: &Path,
    backend: &dyn McpHostBackend,
) -> McpIpcResponse {
    let tag = backend.host_tag();
    let with_tag = |mut v: Value| {
        if let Some(t) = tag {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("host".into(), json!(t));
            }
        }
        v
    };

    match req {
        McpIpcRequest::Ping { token } => {
            if session_token_valid(&token, session_token) {
                McpIpcResponse::success(with_tag(json!({ "pong": true })))
            } else {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            }
        }
        McpIpcRequest::Session { token } => {
            if !session_token_valid(&token, session_token) {
                return McpIpcResponse::failure("unauthorized", "invalid session token");
            }
            match backend.session().await {
                Ok(data) => McpIpcResponse::success(with_tag(json!({
                    "address": data.address,
                    "chain_id": data.chain_id,
                    "network_id": data.network_id,
                }))),
                Err(msg) if msg.contains("locked") => McpIpcResponse::failure("wallet_locked", msg),
                Err(msg) => McpIpcResponse::failure("session_error", msg),
            }
        }
        McpIpcRequest::ProposalStatus { token, proposal_id } => {
            if !session_token_valid(&token, session_token) {
                return McpIpcResponse::failure("unauthorized", "invalid session token");
            }
            if let Err(e) = validate_proposal_id(&proposal_id) {
                return McpIpcResponse::failure(e.code(), e.to_string());
            }
            let queue = ProposalQueue::new(profile_dir);
            match queue.lookup_status(&proposal_id, session_token.as_bytes()) {
                Ok(status) => McpIpcResponse::success(proposal_status_json(&proposal_id, &status)),
                Err(_) => McpIpcResponse::success(json!({
                    "proposal_id": proposal_id,
                    "status": "unknown",
                })),
            }
        }
        McpIpcRequest::Propose {
            token,
            source,
            proposal,
        } => {
            if !session_token_valid(&token, session_token) {
                return McpIpcResponse::failure("unauthorized", "invalid session token");
            }
            if let Err(e) = validate_proposal_id(&proposal.proposal_id) {
                return McpIpcResponse::failure(e.code(), e.to_string());
            }
            match backend.propose(&source, *proposal).await {
                Ok(McpProposeOutcome::Approved { tx_hash }) => {
                    McpIpcResponse::success(with_tag(json!({
                        "status": "approved",
                        "tx_hash": tx_hash,
                    })))
                }
                Ok(McpProposeOutcome::Queued {
                    proposal_id,
                    message,
                }) => McpIpcResponse::success(with_tag(json!({
                    "status": "pending_user",
                    "proposal_id": proposal_id,
                    "message": message,
                }))),
                Ok(McpProposeOutcome::Rejected { reason }) => {
                    McpIpcResponse::failure("user_rejected", reason)
                }
                Err(msg) => McpIpcResponse::failure("exec_failed", msg),
            }
        }
        McpIpcRequest::StealthUri { token } => {
            if !session_token_valid(&token, session_token) {
                return McpIpcResponse::failure("unauthorized", "invalid session token");
            }
            match backend.stealth_uri().await {
                Ok(uri) => McpIpcResponse::success(json!({ "uri": uri })),
                Err(msg) => stealth_failure(msg),
            }
        }
        McpIpcRequest::StealthScan { token } => {
            if !session_token_valid(&token, session_token) {
                return McpIpcResponse::failure("unauthorized", "invalid session token");
            }
            match backend.stealth_scan().await {
                Ok(data) => McpIpcResponse::success(data),
                Err(msg) => stealth_failure(msg),
            }
        }
        McpIpcRequest::StealthSweep {
            token,
            stealth_address,
        } => {
            if !session_token_valid(&token, session_token) {
                return McpIpcResponse::failure("unauthorized", "invalid session token");
            }
            match backend.stealth_sweep(&stealth_address).await {
                Ok(tx_hash) => McpIpcResponse::success(with_tag(json!({
                    "status": "approved",
                    "tx_hash": tx_hash,
                }))),
                Err(msg) => stealth_failure(msg),
            }
        }
    }
}

fn stealth_failure(msg: String) -> McpIpcResponse {
    if msg.starts_with("tui_required") {
        let (_code, rest) = msg
            .split_once(':')
            .unwrap_or(("tui_required", msg.as_str()));
        McpIpcResponse::failure("tui_required", rest.trim())
    } else if msg.contains("offline") || msg.contains("closed") {
        McpIpcResponse::failure("tui_offline", msg)
    } else {
        McpIpcResponse::failure("stealth_error", msg)
    }
}

/// Handle one TCP connection: read request line, dispatch, write response.
pub async fn handle_ipc_connection<B: McpHostBackend>(
    stream: TcpStream,
    session_token: String,
    profile_dir: PathBuf,
    backend: B,
) -> Result<(), String> {
    let (reader, mut writer) = stream.into_split();
    let mut buf = BufReader::new(reader);
    let line = read_ipc_line(&mut buf).await.map_err(|e| e.to_string())?;
    let req: McpIpcRequest = decode_ipc_line(&line).map_err(|e| e.to_string())?;
    let response = dispatch_ipc_request(req, &session_token, &profile_dir, &backend).await;
    let out = encode_line(&response).map_err(|e| e.to_string())?;
    writer
        .write_all(out.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubBackend;

    #[async_trait]
    impl McpHostBackend for StubBackend {
        async fn session(&self) -> Result<McpSessionData, String> {
            Ok(McpSessionData {
                address: "0x1".into(),
                chain_id: 943,
                network_id: "test".into(),
            })
        }

        async fn propose(
            &self,
            _source: &str,
            proposal: TxProposal,
        ) -> Result<McpProposeOutcome, String> {
            Ok(McpProposeOutcome::Queued {
                proposal_id: proposal.proposal_id,
                message: "stub".into(),
            })
        }

        async fn stealth_uri(&self) -> Result<String, String> {
            Ok("st:stub".into())
        }

        async fn stealth_scan(&self) -> Result<Value, String> {
            Ok(json!({ "notes": [] }))
        }

        async fn stealth_sweep(&self, _addr: &str) -> Result<String, String> {
            Ok("0xdead".into())
        }
    }

    #[tokio::test]
    async fn dispatch_rejects_bad_token() {
        let dir = tempfile::tempdir().unwrap();
        let resp = dispatch_ipc_request(
            McpIpcRequest::Ping {
                token: "bad".into(),
            },
            "good-token",
            dir.path(),
            &StubBackend,
        )
        .await;
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn dispatch_rejects_invalid_proposal_id() {
        let dir = tempfile::tempdir().unwrap();
        let resp = dispatch_ipc_request(
            McpIpcRequest::ProposalStatus {
                token: "tok".into(),
                proposal_id: "../evil".into(),
            },
            "tok",
            dir.path(),
            &StubBackend,
        )
        .await;
        assert!(!resp.ok);
    }
}
