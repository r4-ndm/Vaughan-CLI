//! Vaughan MCP stdio server — external agent tool surface.
//!
//! Implements a minimal MCP JSON-RPC 2.0 subset over stdin/stdout. Diagnostics
//! go to stderr only so Cursor's MCP client is not broken by log noise.

pub mod client;
pub mod dispatch;
pub mod server;

pub use server::run_stdio_server;
