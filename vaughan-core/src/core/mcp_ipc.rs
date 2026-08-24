//! Newline-delimited JSON protocol between MCP clients and the TUI listener.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::proposal::TxProposal;

/// Request from an MCP subprocess to the unlocked TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum McpIpcRequest {
    Ping { token: String },
    /// Active wallet session snapshot (address + network) when the TUI is unlocked.
    Session { token: String },
    Propose {
        token: String,
        source: String,
        proposal: Box<TxProposal>,
    },
    ProposalStatus {
        token: String,
        proposal_id: String,
    },
}

/// Response from the TUI listener to an MCP subprocess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpIpcResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpIpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpIpcError {
    pub code: String,
    pub message: String,
}

impl McpIpcResponse {
    pub fn success(data: Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn failure(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(McpIpcError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

/// Encode one IPC message as a single JSON line.
pub fn encode_line<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    Ok(line)
}

/// Decode one newline-terminated JSON line.
pub fn decode_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim())
}
