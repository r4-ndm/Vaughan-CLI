//! Newline-delimited JSON protocol between MCP clients and the TUI listener.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::proposal::TxProposal;

/// Maximum bytes per IPC line (request or response) — rejects oversized payloads.
pub const MCP_IPC_MAX_LINE_BYTES: usize = 2 * 1024 * 1024;

/// Errors decoding an IPC line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpIpcLineError {
    TooLarge,
    Parse(String),
}

impl std::fmt::Display for McpIpcLineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "IPC line exceeds {MCP_IPC_MAX_LINE_BYTES} bytes"),
            Self::Parse(msg) => write!(f, "IPC parse error: {msg}"),
        }
    }
}

impl std::error::Error for McpIpcLineError {}

/// Constant-time session token comparison (loopback hijack mitigation).
pub fn session_token_valid(provided: &str, expected: &str) -> bool {
    constant_time_eq(provided.as_bytes(), expected.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Request from an MCP subprocess to the unlocked TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum McpIpcRequest {
    Ping {
        token: String,
    },
    /// Active wallet session snapshot (address + network) when the TUI is unlocked.
    Session {
        token: String,
    },
    Propose {
        token: String,
        source: String,
        proposal: Box<TxProposal>,
    },
    ProposalStatus {
        token: String,
        proposal_id: String,
    },
    /// Stealth meta-address URI for the unlocked vault.
    StealthUri {
        token: String,
    },
    /// Scan announcer logs for unswept notes owned by this vault.
    StealthScan {
        token: String,
    },
    /// Sweep one note (by stealth address) back to the active account.
    StealthSweep {
        token: String,
        stealth_address: String,
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

/// Decode one newline-terminated JSON line with a size cap.
pub fn decode_ipc_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, McpIpcLineError> {
    if line.len() > MCP_IPC_MAX_LINE_BYTES {
        return Err(McpIpcLineError::TooLarge);
    }
    decode_line(line).map_err(|e| McpIpcLineError::Parse(e.to_string()))
}

/// Decode one newline-terminated JSON line (no size cap — prefer [`decode_ipc_line`]).
pub fn decode_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim())
}
