//! Vaughan MCP stdio server — external agent tool surface.
//!
//! Implements a minimal MCP JSON-RPC 2.0 subset over stdin/stdout. Diagnostics
//! go to stderr only so Cursor's MCP client is not broken by log noise.
//!
//! Full `rmcp` / SDK rewrite is **not scheduled** — see `docs/mcp-transport.md`.

pub mod browser_bridge;
pub mod client;
pub mod dispatch;
pub mod server;
pub mod session_bridge;

pub use dispatch::{McpContext, McpDispatcher};
pub use server::{
    build_context, handle_request, handle_stdio_line, run_stdio_server, RpcErrorObj, RpcRequest,
    RpcResponse, MCP_PROTOCOL_VERSION,
};
