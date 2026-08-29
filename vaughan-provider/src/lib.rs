//! Local EIP-1193 JSON-RPC provider bridge (Phase 2, FR-2.1/FR-2.2).
//!
//! `vaughan-provider` is the loopback-only WebSocket endpoint that lets a
//! dApp browser (VB today; Freedom when upstream PR #195 merges) use Vaughan
//! as its signing wallet. It owns the transport and the JSON-RPC wire protocol
//! but no wallet logic: requests are handed to a host-supplied
//! [`RequestHandler`], which the TUI implements against `vaughan-core` (and
//! which enforces the approval flow for every sign/send request).
//!
//! Module layering:
//! - `error` — EIP-1193 error codes + [`ProviderError`]
//! - `rpc` — JSON-RPC 2.0 request/response wire types
//! - `handler` — the [`RequestHandler`] contract + [`RequestCtx`]
//! - `methods` — EIP-1193 dispatch: [`methods::Eip1193Handler`] + [`methods::WalletHandle`]
//! - `events` — [`events::EventBus`] pushing `accountsChanged`/`chainChanged`
//! - `server` — loopback [`ProviderServer`] (bind, accept, dispatch, relay)
//!
//! # Security properties
//!
//! - The listener binds `127.0.0.1` only, and every accepted peer is
//!   re-verified as loopback before a connection task is spawned.
//! - No secret material is ever logged or placed in error messages.
//! - Signing methods must go through the host's explicit user approval; this
//!   crate never signs anything itself.

pub mod error;
pub mod events;
pub mod handler;
pub mod methods;
pub mod rpc;
pub mod rpc_proxy;
pub mod seal;
pub mod server;

pub use error::ProviderError;
pub use events::{EventBus, ProviderEvent};
pub use handler::{HandlerResult, RequestCtx, RequestHandler};
pub use methods::{Eip1193Handler, TxParams, WalletHandle};
pub use rpc::{RpcError, RpcRequest, RpcResponse};
pub use rpc_proxy::is_read_proxy_method;
pub use server::{ProviderServer, DEFAULT_PORT};
