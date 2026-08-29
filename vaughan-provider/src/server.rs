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

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::{accept_hdr_async_with_config, WebSocketStream};
use url::Url;

use crate::error::ProviderError;
use crate::events::EventBus;
use crate::handler::{RequestCtx, RequestHandler};
use crate::rpc::{RpcError, RpcRequest, RpcResponse};

/// Default port the provider listens on. Must match the Freedom Browser
/// signer backend's default endpoint (`ws://127.0.0.1:8745`).
pub const DEFAULT_PORT: u16 = 8745;

/// Largest accepted JSON-RPC frame (typed-data payloads can be sizable).
const MAX_MESSAGE_SIZE: usize = 4 * 1024 * 1024;

/// Max simultaneous WebSocket connections (loopback DoS guard).
const MAX_CONNECTIONS: usize = 32;

/// The WebSocket handshake must complete within this window (slow-loris guard).
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Per-connection inbound frame cap per [`RATE_WINDOW`] (loopback DoS guard).
const MAX_FRAMES_PER_WINDOW: u32 = 128;
const RATE_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Canonicalized trusted-origin allowlist (FR-2.4).
///
/// When empty, no origin filtering is enforced (legacy-compatible mode).
#[derive(Debug, Clone, Default)]
struct TrustedHosts {
    allowed_origins: HashSet<String>,
}

impl TrustedHosts {
    /// Build from human-entered origin strings (`scheme://host[:port]`).
    fn try_from_origins<I, S>(origins: I) -> Result<Self, ProviderError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut allowed_origins = HashSet::new();
        for raw in origins {
            let raw = raw.as_ref().trim();
            if raw.is_empty() {
                continue;
            }
            let normalized = normalize_origin(raw)?;
            allowed_origins.insert(normalized);
        }
        Ok(Self { allowed_origins })
    }

    fn is_empty(&self) -> bool {
        self.allowed_origins.is_empty()
    }

    /// Whether this origin is permitted to use the provider.
    fn allows(&self, origin: Option<&str>) -> bool {
        if self.is_empty() {
            return true;
        }
        let Some(origin) = origin else {
            return false;
        };
        match normalize_origin(origin) {
            Ok(origin) => self.allowed_origins.contains(&origin),
            Err(_) => false,
        }
    }
}

/// Normalize an origin so matching is stable across case/slash variations.
fn normalize_origin(raw: &str) -> Result<String, ProviderError> {
    let url = Url::parse(raw).map_err(|_| {
        ProviderError::InvalidParams(format!(
            "invalid trusted origin `{raw}` (expected scheme://host[:port])"
        ))
    })?;
    match url.origin() {
        url::Origin::Opaque(_) => {
            // `chrome-extension://<id>` is Opaque in `url`, but Chromium still
            // sends it as the WebSocket Origin for extension service workers.
            if url.scheme() == "chrome-extension" {
                if let Some(host) = url.host_str() {
                    return Ok(format!("chrome-extension://{}", host.to_ascii_lowercase()));
                }
            }
            Err(ProviderError::InvalidParams(format!(
                "invalid trusted origin `{raw}` (origin must include host)"
            )))
        }
        origin => Ok(origin.unicode_serialization()),
    }
}

/// Loopback-only JSON-RPC server.
pub struct ProviderServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    trusted_hosts: TrustedHosts,
    /// When set, every client must present this token — no per-origin
    /// exemptions (Origin is forgeable by any local process). Shared with the
    /// host so the token can be rotated on lock/unlock without restarting
    /// the listener; read fresh at every handshake.
    session_token: std::sync::Arc<std::sync::RwLock<Option<String>>>,
    /// Handshake origins allowed to assert `vaughan_page_origin` on requests
    /// (the attested dApp-browser extension). Empty: the field is ignored.
    page_origin_issuers: Vec<String>,
    /// Per-launch extension seal key (hex, 32 bytes), shared with the host.
    /// When set, issuer connections must prove `vaughan_page_origin` with a
    /// valid AES-GCM seal (`vaughan_origin_seal`); when unset (standalone VB
    /// launch), issuer-only attestation applies (legacy mode).
    origin_seal_key: std::sync::Arc<std::sync::RwLock<Option<String>>>,
    /// Origins currently holding a Connect grant, shared with the host so the
    /// server can filter `accountsChanged` per connection.
    grants: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
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
            trusted_hosts: TrustedHosts::default(),
            session_token: std::sync::Arc::new(std::sync::RwLock::new(None)),
            page_origin_issuers: Vec::new(),
            origin_seal_key: std::sync::Arc::new(std::sync::RwLock::new(None)),
            grants: std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashSet::new())),
        })
    }

    /// Configure the trusted-origin allowlist (FR-2.4).
    ///
    /// Entries must be full origins (`scheme://host[:port]`). When at least one
    /// origin is configured, requests without an `Origin` header are denied.
    pub fn with_trusted_origins<I, S>(mut self, origins: I) -> Result<Self, ProviderError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.trusted_hosts = TrustedHosts::try_from_origins(origins)?;
        Ok(self)
    }

    /// Require clients to present this session token (query `access_token` or
    /// `Authorization: Bearer`). Required for every origin when set.
    pub fn with_session_token(self, token: impl Into<String>) -> Self {
        let t = token.into();
        *self.session_token.write().unwrap() = if t.is_empty() { None } else { Some(t) };
        self
    }

    /// Live session-token slot: rotate or replace the required token while
    /// the server is running (lock/unlock lifecycle). New handshakes read the
    /// slot fresh; existing connections keep their already-authenticated
    /// session. Never set `None` to "disable" auth — `None` means *no* token
    /// required; rotate to a fresh unwritten token instead.
    pub fn session_token_slot(&self) -> std::sync::Arc<std::sync::RwLock<Option<String>>> {
        self.session_token.clone()
    }

    /// Use an externally-owned token slot (host rotates it on lock/unlock).
    pub fn with_session_token_slot(
        mut self,
        slot: std::sync::Arc<std::sync::RwLock<Option<String>>>,
    ) -> Self {
        self.session_token = slot;
        self
    }

    /// Live extension seal-key slot: the host learns the per-launch key from
    /// `vb.session` and installs it here while the server keeps running.
    pub fn with_origin_seal_key_slot(
        mut self,
        slot: std::sync::Arc<std::sync::RwLock<Option<String>>>,
    ) -> Self {
        self.origin_seal_key = slot;
        self
    }

    /// The shared seal-key slot (host side).
    pub fn origin_seal_key_slot(&self) -> std::sync::Arc<std::sync::RwLock<Option<String>>> {
        self.origin_seal_key.clone()
    }

    /// Handshake origins permitted to assert `vaughan_page_origin` on a
    /// request (the attested dApp-browser extension, whose page origin is
    /// derived from Chrome's `port.sender.url`). For all other clients the
    /// field is ignored, so a token-holding script cannot relabel a request
    /// as coming from an arbitrary site.
    pub fn with_page_origin_issuers<I, S>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.page_origin_issuers = origins
            .into_iter()
            .map(|o| o.as_ref().to_string())
            .collect();
        self
    }

    /// Share the host's live Connect-grant set so `accountsChanged` with a
    /// non-empty account list is relayed only to granted origins (and to
    /// page-origin issuers, which route per-tab themselves).
    pub fn with_grants(
        mut self,
        grants: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    ) -> Self {
        self.grants = grants;
        self
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
        let conn_permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
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
            let Ok(permit) = conn_permits.clone().try_acquire_owned() else {
                tracing::warn!(%peer, "provider connection cap reached; dropping");
                drop(stream);
                continue;
            };
            let handler = Arc::clone(&handler);
            let events = events.clone();
            let trusted_hosts = self.trusted_hosts.clone();
            let session_token = self.session_token.clone();
            let page_origin_issuers = self.page_origin_issuers.clone();
            let origin_seal_key = self.origin_seal_key.clone();
            let grants = self.grants.clone();
            tokio::spawn(async move {
                // Held until the connection task ends, releasing the slot.
                let _permit = permit;
                handle_connection(
                    stream,
                    peer,
                    handler,
                    events,
                    trusted_hosts,
                    session_token,
                    page_origin_issuers,
                    origin_seal_key,
                    grants,
                )
                .await;
            });
        }
    }
}

/// Handshake metadata captured before the WebSocket upgrade completes.
#[derive(Default, Clone)]
struct HandshakeMeta {
    origin: Option<String>,
    /// From `Authorization: Bearer` or `?access_token=`.
    access_token: Option<String>,
    /// `Host` header, checked against loopback names for DNS-rebinding
    /// resistance (a rebound domain's browser sends its own Host here).
    host: Option<String>,
}

/// Captures Origin + session token during the WebSocket handshake.
#[derive(Clone)]
struct CaptureHandshake(Arc<Mutex<HandshakeMeta>>);

impl Callback for CaptureHandshake {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        let mut meta = HandshakeMeta::default();
        if let Some(origin) = request
            .headers()
            .get("origin")
            .and_then(|value| value.to_str().ok())
        {
            meta.origin = Some(origin.to_string());
        }
        if let Some(host) = request
            .headers()
            .get("host")
            .and_then(|value| value.to_str().ok())
        {
            meta.host = Some(host.to_string());
        }
        if let Some(auth) = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
        {
            if let Some(token) = auth
                .strip_prefix("Bearer ")
                .or_else(|| auth.strip_prefix("bearer "))
            {
                let t = token.trim();
                if !t.is_empty() {
                    meta.access_token = Some(t.to_string());
                }
            }
        }
        if meta.access_token.is_none() {
            if let Some(query) = request.uri().query() {
                for pair in query.split('&') {
                    if let Some(v) = pair.strip_prefix("access_token=") {
                        let t = v.trim();
                        if !t.is_empty() {
                            meta.access_token = Some(t.to_string());
                            break;
                        }
                    }
                }
            }
        }
        *self.0.lock().expect("handshake mutex poisoned") = meta;
        Ok(response)
    }
}

/// DNS-rebinding guard: the `Host` header must name a loopback interface.
/// A browser pointed at a rebound domain sends that domain as Host, which
/// fails here even if its Origin looks plausible. Missing Host fails closed
/// (HTTP/1.1 requires it; every real WebSocket client sends it).
fn host_is_loopback(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let h = host.trim();
    // Strip an optional port: "[v6]:port" or "name:port"; a value with
    // multiple colons and no brackets is a port-less IPv6 literal.
    let name = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else if h.matches(':').count() == 1 {
        h.split(':').next().unwrap_or("")
    } else {
        h
    };
    name.eq_ignore_ascii_case("localhost")
        || name.to_ascii_lowercase().ends_with(".localhost")
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The session token is required for **every** origin whenever the server has
/// one: the `Origin` header is forgeable by any local process, so it cannot
/// stand in as a credential.
fn session_ok(expected: &Option<String>, presented: Option<&str>) -> bool {
    let Some(exp) = expected.as_deref() else {
        return true;
    };
    let Some(got) = presented else {
        return false;
    };
    constant_time_eq(got, exp)
}

/// Serve one WebSocket connection: handshake, then a read/event loop.
///
/// Incoming requests are dispatched sequentially; events published on
/// `events` are relayed to the client as JSON-RPC notifications.
#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    handler: Arc<dyn RequestHandler>,
    events: EventBus,
    trusted_hosts: TrustedHosts,
    session_token: std::sync::Arc<std::sync::RwLock<Option<String>>>,
    page_origin_issuers: Vec<String>,
    origin_seal_key: std::sync::Arc<std::sync::RwLock<Option<String>>>,
    grants: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
) {
    let handshake = Arc::new(Mutex::new(HandshakeMeta::default()));
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_MESSAGE_SIZE);
    let ws: WebSocketStream<TcpStream> = match tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        accept_hdr_async_with_config(
            stream,
            CaptureHandshake(Arc::clone(&handshake)),
            Some(config),
        ),
    )
    .await
    {
        Ok(Ok(ws)) => ws,
        Ok(Err(e)) => {
            tracing::warn!(%peer, "websocket handshake failed: {e}");
            return;
        }
        Err(_) => {
            tracing::warn!(%peer, "websocket handshake timed out");
            return;
        }
    };
    let meta = handshake.lock().expect("handshake mutex poisoned").clone();
    let origin = meta.origin;
    if !host_is_loopback(meta.host.as_deref()) {
        tracing::warn!(%peer, host = ?meta.host, "rejecting provider connection: non-loopback Host");
        return;
    }
    if !trusted_hosts.allows(origin.as_deref()) {
        tracing::warn!(%peer, ?origin, "rejecting untrusted provider origin");
        return;
    }
    // Read the token slot fresh at handshake time so lock/unlock rotation
    // takes effect without restarting the listener.
    let expected_token = session_token.read().unwrap().clone();
    if !session_ok(&expected_token, meta.access_token.as_deref()) {
        tracing::warn!(
            %peer,
            ?origin,
            "rejecting provider connection: missing/invalid session token"
        );
        return;
    }
    // Only attested issuer origins may assert `vaughan_page_origin`; the
    // session check above has already run, so every accepted connection holds
    // the token.
    let page_origin_allowed = origin
        .as_deref()
        .is_some_and(|o| page_origin_issuers.iter().any(|i| i == o));
    let ctx = RequestCtx {
        peer,
        origin,
        page_origin: None,
    };
    let mut events_rx = events.subscribe();
    tracing::debug!(%peer, "provider client connected");

    let (mut sink, mut incoming) = ws.split();
    let mut rate_window = std::time::Instant::now();
    let mut frames_in_window: u32 = 0;
    loop {
        // Produce at most one outbound frame per iteration; the sink is only
        // borrowed after the select, so the two arms never fight over it.
        let to_send = tokio::select! {
            message = incoming.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    if rate_limit_exceeded(&mut rate_window, &mut frames_in_window) {
                        tracing::warn!(%peer, "provider frame rate limit exceeded; closing");
                        break;
                    }
                    dispatch(&*handler, &ctx, &text, page_origin_allowed, &origin_seal_key)
                        .await
                        .map(|reply| reply.into())
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
                Ok(event) => {
                    if should_relay(&event, ctx.origin.as_deref(), &grants, page_origin_allowed) {
                        Some(Message::Text(event.to_notification().into()))
                    } else {
                        None
                    }
                }
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
async fn dispatch(
    handler: &dyn RequestHandler,
    ctx: &RequestCtx,
    text: &str,
    page_origin_allowed: bool,
    origin_seal_key: &std::sync::RwLock<Option<String>>,
) -> Option<String> {
    let request = match RpcRequest::from_json(text) {
        Ok(request) => request,
        Err(rpc_error) => return Some(RpcResponse::failure(None, rpc_error).to_json()),
    };
    let id = request.id.clone();
    let mut req_ctx = ctx.clone();
    if let Some(page) = request.vaughan_page_origin.clone() {
        let page = page.trim().to_string();
        if !page.is_empty() {
            if page_origin_allowed {
                // When a per-launch seal key is installed, the assertion must
                // carry a valid AES-GCM seal of the origin — the handshake
                // Origin header alone is forgeable by any token-holding local
                // process. No key configured = standalone VB (legacy mode).
                let key = origin_seal_key.read().unwrap().clone();
                if let Some(key) = key {
                    let ok = request
                        .vaughan_origin_seal
                        .as_deref()
                        .is_some_and(|seal| crate::seal::verify_origin_seal(&key, seal, &page));
                    if !ok {
                        tracing::warn!(
                            origin = ?ctx.origin,
                            "rejecting vaughan_page_origin with missing/invalid origin seal"
                        );
                        if request.is_notification() {
                            return None;
                        }
                        return Some(
                            RpcResponse::failure(
                                id,
                                provider_error_to_rpc(ProviderError::Unauthorized(
                                    "page-origin assertion failed seal verification".into(),
                                )),
                            )
                            .to_json(),
                        );
                    }
                }
                req_ctx.page_origin = Some(page);
            } else {
                tracing::warn!(
                    origin = ?ctx.origin,
                    "ignoring vaughan_page_origin from non-issuer origin"
                );
            }
        }
    }
    if request.is_notification() {
        let _ = handler.handle(req_ctx, request).await;
        return None;
    }
    match handler.handle(req_ctx, request).await {
        Ok(result) => Some(RpcResponse::success(id, result).to_json()),
        Err(provider_error) => {
            Some(RpcResponse::failure(id, provider_error_to_rpc(provider_error)).to_json())
        }
    }
}

/// Simple per-connection frame rate limit: returns true when the caller
/// exceeds [`MAX_FRAMES_PER_WINDOW`] within [`RATE_WINDOW`].
fn rate_limit_exceeded(window_start: &mut std::time::Instant, frames: &mut u32) -> bool {
    if window_start.elapsed() >= RATE_WINDOW {
        *window_start = std::time::Instant::now();
        *frames = 0;
    }
    *frames += 1;
    *frames > MAX_FRAMES_PER_WINDOW
}

/// Whether an event should be relayed to this connection.
///
/// Non-empty `accountsChanged` goes only to origins holding a Connect grant,
/// or to a page-origin issuer (the extension multiplexes many pages over one
/// socket and routes per-tab itself). The empty (lock/disconnect) event is
/// broadcast to everyone so all clients clear state. `chainChanged` is not
/// account data and is always relayed.
fn should_relay(
    event: &crate::events::ProviderEvent,
    origin: Option<&str>,
    grants: &std::sync::RwLock<std::collections::HashSet<String>>,
    page_origin_issuer: bool,
) -> bool {
    match event {
        crate::events::ProviderEvent::AccountsChanged(accounts) if !accounts.is_empty() => {
            page_origin_issuer
                || origin.is_some_and(|o| grants.read().map(|g| g.contains(o)).unwrap_or(false))
        }
        _ => true,
    }
}

/// Convert a handler error into its wire form.
///
/// `Internal` detail (paths, backend errors) is logged, not sent — the wire
/// gets a generic message; typed errors keep their messages since those are
/// part of the EIP-1193 contract.
fn provider_error_to_rpc(error: ProviderError) -> RpcError {
    let code = error.code();
    match error {
        ProviderError::Internal(detail) => {
            tracing::debug!("provider internal error: {detail}");
            RpcError::new(code, "internal error".to_string(), None)
        }
        other => RpcError::new(other.code(), other.to_string(), None),
    }
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

    type GrantsHandle = std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>;

    async fn start_server(
        handler: Arc<dyn RequestHandler>,
        events: Option<EventBus>,
        trusted_origins: Option<Vec<&str>>,
    ) -> (JoinHandle, String, EventBus, GrantsHandle) {
        let server = ProviderServer::bind(0).await.unwrap();
        let server = match trusted_origins {
            Some(origins) => server.with_trusted_origins(origins).unwrap(),
            None => server,
        };
        let grants: GrantsHandle = Default::default();
        let server = server.with_grants(grants.clone());
        let url = server.url();
        let events = events.unwrap_or_default();
        let task = tokio::spawn(server.serve(handler, events.clone()));
        (task, url, events, grants)
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

    #[test]
    fn trusted_origins_reject_bad_entries() {
        let server = ProviderServer::bind(0);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(server).unwrap();
        let err = server.with_trusted_origins(["not-an-origin"]);
        assert!(err.is_err(), "must reject invalid origin");
        let err = err.err().unwrap();
        assert_eq!(err.code(), crate::error::codes::INVALID_PARAMS);
    }

    #[test]
    fn chrome_extension_origin_is_accepted() {
        let normalized =
            super::normalize_origin("chrome-extension://cneeaoilhnioopaiaidjadinahpgacpn")
                .expect("chrome-extension origins must normalize");
        assert_eq!(
            normalized,
            "chrome-extension://cneeaoilhnioopaiaidjadinahpgacpn"
        );
        let hosts = TrustedHosts::try_from_origins([normalized.as_str()]).unwrap();
        assert!(hosts.allows(Some("chrome-extension://cneeaoilhnioopaiaidjadinahpgacpn")));
    }

    #[tokio::test]
    async fn answers_request_over_real_websocket() {
        let (handler, erased) = recording_handler(None);
        let (task, url, _events, _grants) = start_server(erased, None, None).await;

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
        let (task, url, _events, _grants) = start_server(erased, None, None).await;

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
        let (task, url, _events, _grants) = start_server(erased, None, None).await;

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
        let (task, url, _events, _grants) = start_server(erased, None, None).await;

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
        let (task, url, _events, _grants) = start_server(erased, None, None).await;

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
        let (task, url, _events, _grants) = start_server(erased, None, None).await;

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
        let (task, url, events, grants) = start_server(erased, None, None).await;

        // Non-empty accountsChanged requires a Connect grant for the origin.
        grants
            .write()
            .unwrap()
            .insert("https://app.example".to_string());
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", "https://app.example".parse().unwrap());
        let (mut ws, _) = connect_async(request).await.unwrap();
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

    #[tokio::test]
    async fn accounts_changed_filtered_without_grant() {
        let (_, erased) = recording_handler(None);
        let (task, url, events, _grants) = start_server(erased, None, None).await;

        // No grant for this origin: non-empty accountsChanged is withheld,
        // chainChanged and the empty (lock) broadcast still arrive.
        let mut ws = connect(&url).await;
        events.publish(ProviderEvent::AccountsChanged(vec!["0xabc".into()]));
        events.publish(ProviderEvent::ChainChanged("0x171".into()));
        events.publish(ProviderEvent::AccountsChanged(vec![]));

        let first = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let value = parse_reply(first);
        assert_eq!(value["method"], "chainChanged");

        let second = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let value = parse_reply(second);
        assert_eq!(value["method"], "accountsChanged");
        assert_eq!(value["params"], serde_json::json!([]));
        task.abort();
    }

    #[tokio::test]
    async fn rejects_missing_origin_when_allowlist_enabled() {
        let (_, erased) = recording_handler(None);
        let (task, url, _events, _grants) =
            start_server(erased, None, Some(vec!["https://app.example"])).await;

        let mut ws = connect(&url).await;
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId"}"#.into(),
        ))
        .await
        .unwrap();
        let next = timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .expect("socket closes");
        assert!(matches!(next, Ok(Message::Close(_)) | Err(_)));
        task.abort();
    }

    #[tokio::test]
    async fn allows_trusted_origin_when_allowlist_enabled() {
        let (handler, erased) = recording_handler(None);
        let (task, url, _events, _grants) =
            start_server(erased, None, Some(vec!["https://app.example"])).await;

        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", "https://app.example/".parse().unwrap());
        let (mut ws, _) = connect_async(request).await.unwrap();
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
        assert_eq!(handler.exchanges().lock().unwrap().len(), 1);
        task.abort();
    }

    #[tokio::test]
    async fn rejects_untrusted_origin_when_allowlist_enabled() {
        let (_, erased) = recording_handler(None);
        let (task, url, _events, _grants) =
            start_server(erased, None, Some(vec!["https://allowed.example"])).await;

        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", "https://evil.example".parse().unwrap());
        let (mut ws, _) = connect_async(request).await.unwrap();
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId"}"#.into(),
        ))
        .await
        .unwrap();
        let next = timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .expect("socket closes");
        assert!(matches!(next, Ok(Message::Close(_)) | Err(_)));
        task.abort();
    }

    #[tokio::test]
    async fn rejects_non_loopback_host_header() {
        let (_, erased) = recording_handler(None);
        let (task, url, _events, _grants) = start_server(erased, None, None).await;

        // DNS-rebinding: a browser driven to a rebound domain sends that
        // domain as Host even though the socket landed on 127.0.0.1.
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Host", "rebound.example".parse().unwrap());
        let (mut ws, _) = connect_async(request).await.unwrap();
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId"}"#.into(),
        ))
        .await
        .unwrap();
        let next = timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .expect("socket closes");
        assert!(matches!(next, Ok(Message::Close(_)) | Err(_)));
        task.abort();
    }

    #[test]
    fn host_is_loopback_check() {
        assert!(host_is_loopback(Some("127.0.0.1:8745")));
        assert!(host_is_loopback(Some("127.0.0.2")));
        assert!(host_is_loopback(Some("localhost:8745")));
        assert!(host_is_loopback(Some("foo.localhost:8745")));
        assert!(host_is_loopback(Some("[::1]:8745")));
        assert!(host_is_loopback(Some("::1")));
        assert!(!host_is_loopback(Some("rebound.example")));
        // Browser shorthand forms of 127.0.0.1 fail closed (rejected).
        assert!(!host_is_loopback(Some("2130706433")));
        assert!(!host_is_loopback(Some("127.1")));
        assert!(!host_is_loopback(None));
        assert!(!host_is_loopback(Some("")));
    }
}
