//! MCP JSON-RPC 2.0 stdio server (minimal Cursor-compatible subset).
//!
//! ## Claimed protocol subset
//!
//! Framing: **one JSON object per newline** on stdin/stdout (no Content-Length).
//! Methods: `initialize`, `notifications/initialized` / `initialized`, `ping`,
//! `tools/list`, `tools/call`. Diagnostics go to **stderr only**.
//!
//! Hosts that require Content-Length framing are out of scope until documented.
//! Do **not** start an `rmcp` rewrite without the revisit triggers in
//! `docs/mcp-transport.md`.

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing_subscriber::EnvFilter;
use vaughan_core::chains::evm::networks::get_network_by_id;
use vaughan_core::core::persistence::StateManager;

use crate::dispatch::{McpContext, McpDispatcher};

const SERVER_NAME: &str = "vaughan-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Protocol version advertised in `initialize` (MCP 2024-11-05).
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
pub struct RpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcErrorObj>,
}

#[derive(Debug, Serialize)]
pub struct RpcErrorObj {
    pub code: i32,
    pub message: String,
}

/// Run the MCP stdio server until stdin closes.
pub async fn run_stdio_server(profile: String, source: String) -> io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();

    let dispatcher = McpDispatcher::new(&profile).map_err(io::Error::other)?;
    let ctx = build_context(&profile, &source).map_err(io::Error::other)?;

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_stdio_line(&dispatcher, &ctx, &line).await;
        let out = serde_json::to_string(&response).map_err(io::Error::other)?;
        writeln!(stdout, "{out}")?;
        stdout.flush()?;
    }
    Ok(())
}

/// Parse one newline-delimited JSON-RPC request and produce a response.
///
/// Used by the stdio loop and by conformance tests (no stdin required).
pub async fn handle_stdio_line(
    dispatcher: &McpDispatcher,
    ctx: &McpContext,
    line: &str,
) -> RpcResponse {
    match serde_json::from_str::<RpcRequest>(line) {
        Ok(req) => handle_request(dispatcher, ctx, req).await,
        Err(e) => RpcResponse {
            jsonrpc: "2.0",
            id: Value::Null,
            result: None,
            error: Some(RpcErrorObj {
                code: -32700,
                message: format!("parse error: {e}"),
            }),
        },
    }
}

pub async fn handle_request(
    dispatcher: &McpDispatcher,
    ctx: &McpContext,
    req: RpcRequest,
) -> RpcResponse {
    let id = req.id.unwrap_or(Value::Null);
    match req.method.as_str() {
        "initialize" => RpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION,
                }
            })),
            error: None,
        },
        "notifications/initialized" | "initialized" => RpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({})),
            error: None,
        },
        "ping" => RpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({})),
            error: None,
        },
        "tools/list" => RpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({ "tools": dispatcher.tool_definitions() })),
            error: None,
        },
        "tools/call" => {
            let name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match dispatcher.call_tool(name, args, ctx).await {
                Ok(data) => RpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: Some(json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&data).unwrap_or_default()
                        }],
                        "isError": false
                    })),
                    error: None,
                },
                Err(msg) => RpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: Some(json!({
                        "content": [{
                            "type": "text",
                            "text": msg
                        }],
                        "isError": true
                    })),
                    error: None,
                },
            }
        }
        _ => RpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcErrorObj {
                code: -32601,
                message: format!("method not found: {}", req.method),
            }),
        },
    }
}

/// Build MCP context for a profile (testnet default when vault missing).
pub fn build_context(profile: &str, source: &str) -> Result<McpContext, String> {
    let sm = StateManager::for_profile(profile).map_err(|e| e.user_message())?;
    let net_id = if sm.exists() {
        sm.load().map_err(|e| e.user_message())?.active_network_id
    } else {
        "pulsechain-testnet-v4".to_string()
    };
    let net = get_network_by_id(&net_id).ok_or_else(|| format!("unknown network: {net_id}"))?;
    Ok(McpContext {
        profile: profile.to_string(),
        rpc_url: net.rpc_url.clone(),
        chain_id: net.chain_id,
        network_id: net.id.clone(),
        is_testnet: net.is_testnet,
        active_address: None,
        source: source.to_string(),
    })
}
