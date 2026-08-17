//! The loopback-only EIP-1193 WebSocket server (FR-2.1).
//!
//! [`ProviderServer`] binds a TCP listener on `127.0.0.1`, accepts WebSocket
//! connections, and serves JSON-RPC requests from a single host-supplied
//! [`RequestHandler`]. Loopback is enforced twice: the listener only ever
//! binds the loopback address, and each accepted peer is re-checked before
//! its connection task is spawned (defense in depth).
//!
//! Each connection is handled by its own tokio task with a serialized
//! read→dispatch→write loop, so requests on one connection are answered in
//! order — which is what EIP-1193 clients expect. Framing errors (bad JSON,
//! non-object payloads) are answered with JSON-RPC error responses and the
//! connection stays open; only transport failures close it.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::{accept_hdr_async_with_config, WebSocketStream};

use crate::error::ProviderError;
use crate::events::EventBus;
use crate::handler::{RequestCtx, RequestHandler};
use crate::rpc::{RpcError, RpcRequest, RpcResponse};

/// Default port the provider listens on. Must match the Freedom Browser
/// signer backend's default endpoint (`ws://127.0.0.1:8745`).
pub const DEFAULT_PORT: u16 = 8745;

/// Largest accepted JSON-RPC frame (typed-data payloads can be sizable).
const MAX_MESSAGE_SIZE: usize = 4 * 1024 * 1024;

/// Loopback-only JSON-RPC server.
pub struct ProviderServer {
    listener: TcpListener,
    local_addr: SocketAddr,
}

impl ProviderServer {
    /// Bind a listener on `127.0.0.1:port` (loopback only, FR-2.1).
    ///
    /// Pass `0` to let the OS pick a free port; read it back via
    /// [`Self::local_addr`].
    pub async fn bind(port: u16) -> Result<Self, ProviderError> {
        let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        Ok(Self {
            listener,
            local_addr,
        })
    }

    /// The bound address (useful after binding port `0`).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The `ws://127.0.0.1:<port>` URL clients connect to.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.local_addr.port())
    }

    /// Accept connections forever, dispatching requests to `handler` and
    /// relaying `events` to every connected client.
    ///
    /// Returns only when the listener fails; the caller decides when to stop
    /// (e.g. by aborting the task that runs this future).
    pub async fn serve(
        self,
        handler: Arc<dyn RequestHandler>,
        events: EventBus,
    ) -> Result<(), ProviderError> {
        loop {
            let (stream, peer) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(e) => return Err(ProviderError::from(e)),
            };
            // Re-check the peer: only loopback clients may use the bridge.
            if !peer.ip().is_loopback() {
                tracing::warn!(%peer, "rejecting non-loopback provider connection");
                drop(stream);
                continue;
            }
            let handler = Arc::clone(&handler);
            let events = events.clone();
            tokio::spawn(async move {
                handle_connection(stream, peer, handler, events).await;
            });
        }
    }
}

/// Captures the `Origin` header during the WebSocket handshake.
///
/// The handshake callback runs synchronously inside tungstenite; we stash the
/// value behind an `Arc` and read it once the handshake completes.
#[derive(Clone)]
struct CaptureOrigin(Arc<Mutex<Option<String>>>);

impl Callback for CaptureOrigin {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        let origin = request
            .headers()
            .get("origin")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        if let Some(origin) = origin {
            *self.0.lock().expect("origin capture mutex poisoned") = Some(origin);
        }
        Ok(response)
    }
}

/// Serve one WebSocket connection: handshake, then a read/event loop.
///
/// Incoming requests are dispatched sequentially; events published on
/// `events` are relayed to the client as JSON-RPC notifications.
async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    handler: Arc<dyn RequestHandler>,
    events: EventBus,
) {
    let origin = Arc::new(Mutex::new(None::<String>));
    // `WebSocketConfig` is `#[non_exhaustive]`, so mutate the defaults.
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_MESSAGE_SIZE);
    let ws: WebSocketStream<TcpStream> = match accept_hdr_async_with_config(
        stream,
        CaptureOrigin(Arc::clone(&origin)),
        Some(config),
    )
    .await
    {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!(%peer, "websocket handshake failed: {e}");
            return;
        }
    };
    let origin = origin
        .lock()
        .expect("origin capture mutex poisoned")
        .clone();
    let ctx = RequestCtx { peer, origin };
    let mut events_rx = events.subscribe();
    tracing::debug!(%peer, "provider client connected");

    let (mut sink, mut incoming) = ws.split();
    loop {
        // Produce at most one outbound frame per iteration; the sink is only
        // borrowed after the select, so the two arms never fight over it.
        let to_send = tokio::select! {
            message = incoming.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    dispatch(&*handler, &ctx, &text).await.map(|reply| reply.into())
                }
                Some(Ok(Message::Ping(payload))) => Some(Message::Pong(payload)),
                Some(Ok(Message::Binary(_))) => {
                    // EIP-1193 is JSON-RPC over text frames only.
                    let error = RpcError::invalid_request("binary frames are not supported");
                    Some(Message::Text(RpcResponse::failure(None, error).to_json().into()))
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Frame(_) | Message::Pong(_))) => None,
                Some(Err(e)) => {
                    tracing::debug!(%peer, "websocket read error: {e}");
                    break;
                }
            },
            event = events_rx.recv() => match event {
                Ok(notification) => Some(Message::Text(notification.into())),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(%peer, skipped, "provider client missed events");
                    None
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
        };
        if let Some(message) = to_send {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    }
    tracing::debug!(%peer, "provider client disconnected");
}

/// Parse one text frame, dispatch it, and serialize the reply.
///
/// Returns `None` for notifications (no reply expected) and for replies that
/// could not be serialized (which only happens on programmer error).
async fn dispatch(handler: &dyn RequestHandler, ctx: &RequestCtx, text: &str) -> Option<String> {
    let request = match RpcRequest::from_json(text) {
        Ok(request) => request,
        Err(rpc_error) => return Some(RpcResponse::failure(None, rpc_error).to_json()),
    };
    let id = request.id.clone();
    if request.is_notification() {
        // Notifications are dispatched but never answered.
        let _ = handler.handle(ctx.clone(), request).await;
        return None;
    }
    match handler.handle(ctx.clone(), request).await {
        Ok(result) => Some(RpcResponse::success(id, result).to_json()),
        Err(provider_error) => {
            Some(RpcResponse::failure(id, provider_error_to_rpc(provider_error)).to_json())
        }
    }
}

/// Convert a handler error into its wire form.
fn provider_error_to_rpc(error: ProviderError) -> RpcError {
    RpcError::new(error.code(), error.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ProviderEvent;
    use serde_json::Value;
    use std::time::Duration;
    use tokio::time::timeout;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    /// A request/response exchange captured by the recording handler.
    struct Exchange {
        peer: SocketAddr,
        origin: Option<String>,
        method: String,
    }

    /// Test handler that records every request and answers with a canned value.
    struct RecordingHandler {
        exchanges: Arc<Mutex<Vec<Exchange>>>,
        fail_with: Option<ProviderError>,
    }

    impl RecordingHandler {
        fn exchanges(&self) -> Arc<Mutex<Vec<Exchange>>> {
            Arc::clone(&self.exchanges)
        }
    }

    #[async_trait::async_trait]
    impl RequestHandler for RecordingHandler {
        async fn handle(
            &self,
            ctx: RequestCtx,
            request: RpcRequest,
        ) -> Result<Value, ProviderError> {
            self.exchanges
                .lock()
                .expect("test mutex poisoned")
                .push(Exchange {
                    peer: ctx.peer,
                    origin: ctx.origin,
                    method: request.method.clone(),
                });
            match &self.fail_with {
                Some(error) => Err(error.clone()),
                None => Ok(Value::String(format!("ok:{}", request.method))),
            }
        }
    }

    fn recording_handler(
        fail_with: Option<ProviderError>,
    ) -> (Arc<RecordingHandler>, Arc<dyn RequestHandler>) {
        let handler = Arc::new(RecordingHandler {
            exchanges: Arc::new(Mutex::new(Vec::new())),
            fail_with,
        });
        let erased: Arc<dyn RequestHandler> = handler.clone();
        (handler, erased)
    }

    async fn start_server(
        handler: Arc<dyn RequestHandler>,
        events: Option<EventBus>,
    ) -> (JoinHandle, String, EventBus) {
        let server = ProviderServer::bind(0).await.unwrap();
        let url = server.url();
        let events = events.unwrap_or_default();
        let task = tokio::spawn(server.serve(handler, events.clone()));
        (task, url, events)
    }

    type JoinHandle = tokio::task::JoinHandle<Result<(), ProviderError>>;
    type TestSocket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    async fn connect(url: &str) -> TestSocket {
        let (ws, _) = connect_async(url).await.unwrap();
        ws
    }

    fn parse_reply(message: Message) -> Value {
        let Message::Text(text) = message else {
            panic!("expected a text reply, got {message:?}");
        };
        serde_json::from_str(&text).unwrap()
    }

    #[tokio::test]
    async fn binds_loopback_only() {
        let server = ProviderServer::bind(0).await.unwrap();
        let addr = server.local_addr();
        assert!(addr.ip().is_loopback(), "bound address must be loopback");
        assert_eq!(server.url(), format!("ws://127.0.0.1:{}", addr.port()));
    }

    #[tokio::test]
    async fn answers_request_over_real_websocket() {
        let (handler, erased) = recording_handler(None);
        let (task, url, _events) = start_server(erased, None).await;

        let mut ws = connect(&url).await;
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId"}"#.into(),
        ))
        .await
        .unwrap();
        let reply = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let value = parse_reply(reply);
        assert_eq!(value["id"], 1);
        assert_eq!(value["result"], "ok:eth_chainId");

        // The handler saw the connection context.
        let seen = handler
            .exchanges()
            .lock()
            .unwrap()
            .pop()
            .expect("handler must have seen the request");
        assert_eq!(seen.method, "eth_chainId");
        assert!(seen.peer.ip().is_loopback());

        task.abort();
    }

    #[tokio::test]
    async fn echoes_origin_header() {
        let (handler, erased) = recording_handler(None);
        let (task, url, _events) = start_server(erased, None).await;

        // Build from the URL so tungstenite fills in the handshake headers,
        // then add the Origin header the server should capture.
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", "https://app.example".parse().unwrap());
        let (mut ws, _) = connect_async(request).await.unwrap();
        ws.send(Message::Text(r#"{"id":2,"method":"eth_accounts"}"#.into()))
            .await
            .unwrap();
        let _ = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        let seen = handler
            .exchanges()
            .lock()
            .unwrap()
            .pop()
            .expect("handler must have seen the request");
        assert_eq!(seen.origin.as_deref(), Some("https://app.example"));
        task.abort();
    }
    #[tokio::test]
    async fn answers_errors_with_matching_id() {
        let (_, erased) =
            recording_handler(Some(ProviderError::Unauthorized("unknown origin".into())));
        let (task, url, _events) = start_server(erased, None).await;

        let mut ws = connect(&url).await;
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":"abc","method":"eth_accounts"}"#.into(),
        ))
        .await
        .unwrap();
        let reply = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let value = parse_reply(reply);
        assert_eq!(value["id"], "abc");
        assert_eq!(value["error"]["code"], 4100);
        task.abort();
    }

    #[tokio::test]
    async fn replies_to_malformed_frames_with_parse_error() {
        let (_, erased) = recording_handler(None);
        let (task, url, _events) = start_server(erased, None).await;

        let mut ws = connect(&url).await;
        ws.send(Message::Text("{not json".into())).await.unwrap();
        let reply = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let value = parse_reply(reply);
        assert_eq!(value["error"]["code"], -32700);
        task.abort();
    }

    #[tokio::test]
    async fn notifications_get_no_reply() {
        let (handler, erased) = recording_handler(None);
        let (task, url, _events) = start_server(erased, None).await;

        let mut ws = connect(&url).await;
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","method":"eth_accounts"}"#.into(),
        ))
        .await
        .unwrap();
        // The handler still executed, but nothing comes back over the socket.
        let silence = timeout(Duration::from_millis(300), ws.next()).await;
        assert!(silence.is_err(), "notifications must not be answered");
        assert_eq!(
            handler.exchanges().lock().unwrap().len(),
            1,
            "notification must still be dispatched"
        );
        task.abort();
    }

    #[tokio::test]
    async fn keeps_connection_open_after_request() {
        let (_, erased) = recording_handler(None);
        let (task, url, _events) = start_server(erased, None).await;

        let mut ws = connect(&url).await;
        for id in 1..=3 {
            ws.send(Message::Text(
                format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"eth_chainId"}}"#).into(),
            ))
            .await
            .unwrap();
            let reply = timeout(Duration::from_secs(5), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let value = parse_reply(reply);
            assert_eq!(value["id"], id);
        }
        ws.send(Message::Close(None)).await.unwrap();
        task.abort();
    }

    #[tokio::test]
    async fn relays_events_to_connected_clients() {
        let (_, erased) = recording_handler(None);
        let (task, url, events) = start_server(erased, None).await;

        let mut ws = connect(&url).await;
        // Publish after the connection is established.
        events.publish(ProviderEvent::AccountsChanged(vec!["0xabc".into()]));
        events.publish(ProviderEvent::ChainChanged("0x171".into()));

        let first = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let value = parse_reply(first);
        assert_eq!(value["method"], "accountsChanged");
        assert_eq!(value["params"][0], "0xabc");
        assert!(value.get("id").is_none());

        let second = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let value = parse_reply(second);
        assert_eq!(value["method"], "chainChanged");
        assert_eq!(value["params"], "0x171");
        task.abort();
    }
}
