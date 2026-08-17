//! The host-facing request handler contract.
//!
//! The server owns the transport and JSON-RPC framing; everything else is
//! delegated to a [`RequestHandler`] supplied by the host application (the
//! TUI). A handler receives each validated request with its [`RequestCtx`]
//! (who connected, and from which origin) and returns the result value or a
//! [`ProviderError`].
//!
//! Because the host may need to pause a request for a user approval prompt
//! (every sign/send must be approved), `handle` is async and may take as
//! long as the user needs; the per-connection task simply awaits it.

use std::net::SocketAddr;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ProviderError;
use crate::rpc::RpcRequest;

/// The result of handling one request: the response value, or an error.
pub type HandlerResult = Result<Value, ProviderError>;

/// Metadata about the client that sent a request.
#[derive(Debug, Clone)]
pub struct RequestCtx {
    /// The client's socket address. The server only ever accepts loopback
    /// connections, so this is always a 127.0.0.0/8 (or ::1) address.
    pub peer: SocketAddr,
    /// The `Origin` header from the WebSocket handshake, if the client sent
    /// one. The trusted-host allowlist (FR-2.4) enforces this; clients that
    /// send no origin (e.g. the Freedom Browser backend) are keyed by peer.
    pub origin: Option<String>,
}

/// Handles validated JSON-RPC requests from connected clients.
///
/// Implementations must be cheaply cloneable via `Arc` and safe to call from
/// any connection task. Read-only wallet queries may be answered directly;
/// signing methods must funnel through the host's approval flow before
/// touching key material.
#[async_trait]
pub trait RequestHandler: Send + Sync + 'static {
    /// Handle `request` from `ctx` and produce a result value or an error.
    async fn handle(&self, ctx: RequestCtx, request: RpcRequest) -> HandlerResult;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct PingHandler;

    #[async_trait]
    impl RequestHandler for PingHandler {
        async fn handle(&self, _ctx: RequestCtx, request: RpcRequest) -> HandlerResult {
            Ok(Value::String(format!("pong:{}", request.method)))
        }
    }

    #[tokio::test]
    async fn handler_trait_is_object_safe_and_callable() {
        let handler: Arc<dyn RequestHandler> = Arc::new(PingHandler);
        let ctx = RequestCtx {
            peer: "127.0.0.1:1234".parse().unwrap(),
            origin: None,
        };
        let request = RpcRequest::from_json(r#"{"id":1,"method":"ping"}"#).unwrap();
        let result = handler.handle(ctx, request).await.unwrap();
        assert_eq!(result, Value::String("pong:ping".into()));
    }
}
