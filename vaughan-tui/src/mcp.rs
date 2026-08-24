//! MCP control-plane listener — loopback TCP for external agent proposals.
//!
//! Mirrors the EIP-1193 provider pattern: the listener runs on a tokio task and
//! forwards propose requests to the UI thread over an MPSC channel.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use vaughan_core::core::mcp_ipc::{decode_line, encode_line, McpIpcRequest, McpIpcResponse};
use vaughan_core::core::proposal::{
    mcp_control_port, ProposalQueue, TxProposal, McpSessionToken,
};
use vaughan_provider::ProviderError;

/// Live session metadata exposed to MCP clients while the wallet is unlocked.
#[derive(Debug, Clone, Default)]
pub struct McpSessionSnapshot {
    pub address: Option<String>,
    pub chain_id: Option<u64>,
    pub network_id: Option<String>,
}

/// Request forwarded from the MCP listener to the UI thread.
pub enum McpHostRequest {
    Propose {
        proposal: Box<TxProposal>,
        source: String,
        /// `Some` when a live MCP client is waiting; `None` for file-queue surfacing.
        reply: Option<oneshot::Sender<Result<String, ProviderError>>>,
    },
}

struct ListenerState {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ListenerState {
    fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }
}

/// Manages MCP session token + background listener lifecycle.
pub struct McpService {
    profile_dir: std::path::PathBuf,
    session: Option<McpSessionToken>,
    listener: ListenerState,
    requests: mpsc::UnboundedSender<McpHostRequest>,
    surfaced: HashSet<String>,
    snapshot: Arc<RwLock<McpSessionSnapshot>>,
}

impl McpService {
    pub fn new(profile_dir: &Path, requests: mpsc::UnboundedSender<McpHostRequest>) -> Self {
        Self {
            profile_dir: profile_dir.to_path_buf(),
            session: None,
            listener: ListenerState::new(),
            requests,
            surfaced: HashSet::new(),
            snapshot: Arc::new(RwLock::new(McpSessionSnapshot::default())),
        }
    }

    /// Update the session snapshot the loopback listener serves via `Session` IPC.
    pub fn update_session(&self, snapshot: McpSessionSnapshot) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = snapshot;
        }
    }

    /// Start session token + loopback listener when wallet unlocks.
    pub fn on_unlock(&mut self, runtime: &tokio::runtime::Handle) {
        if self.session.is_some() {
            return;
        }
        let token = McpSessionToken::generate();
        if token.write(&self.profile_dir).is_err() {
            return;
        }
        self.listener.stop.store(false, Ordering::SeqCst);
        let stop = self.listener.stop.clone();
        let tx = self.requests.clone();
        let session = token.as_str().to_string();
        let profile_dir = self.profile_dir.clone();
        let snapshot = self.snapshot.clone();
        let handle = runtime.spawn(async move {
            if let Err(e) = run_listener(stop, tx, session, profile_dir, snapshot).await {
                tracing::warn!("mcp listener stopped: {e}");
            }
        });
        self.listener.handle = Some(handle);
        self.session = Some(token);
    }

    /// Stop listener and invalidate session token when wallet locks.
    pub fn on_lock(&mut self) {
        self.listener.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.listener.handle.take() {
            handle.abort();
        }
        self.session = None;
        self.surfaced.clear();
        self.update_session(McpSessionSnapshot::default());
        let _ = McpSessionToken::invalidate(&self.profile_dir);
    }

    pub fn session_secret(&self) -> Option<&str> {
        self.session.as_ref().map(|t| t.as_str())
    }

    /// Poll file queue and surface the oldest not-yet-shown pending proposal.
    pub fn poll_file_queue(&mut self, pending_on_screen: bool) {
        if pending_on_screen {
            return;
        }
        let Some(secret) = self.session_secret() else {
            return;
        };
        let queue = ProposalQueue::new(&self.profile_dir);
        let _ = queue.sweep_expired();
        let Ok(pending) = queue.list_pending() else {
            return;
        };
        for queued in pending {
            let id = queued.proposal.proposal_id.clone();
            if self.surfaced.contains(&id) {
                continue;
            }
            if queue
                .get_pending(&id, secret.as_bytes())
                .is_err()
            {
                continue;
            }
            self.surfaced.insert(id.clone());
            let _ = self.requests.send(McpHostRequest::Propose {
                proposal: Box::new(queued.proposal),
                source: queued.source,
                reply: None,
            });
            break;
        }
    }
}

async fn run_listener(
    stop: Arc<AtomicBool>,
    ui_tx: mpsc::UnboundedSender<McpHostRequest>,
    session_token: String,
    profile_dir: std::path::PathBuf,
    snapshot: Arc<RwLock<McpSessionSnapshot>>,
) -> Result<(), String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, mcp_control_port()));
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))?;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let accept = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            listener.accept(),
        )
        .await;
        let Ok(Ok((stream, _))) = accept else {
            continue;
        };
        let ui_tx = ui_tx.clone();
        let token = session_token.clone();
        let profile_dir = profile_dir.clone();
        let snapshot = snapshot.clone();
        tokio::spawn(async move {
            let _ = handle_connection(stream, ui_tx, token, profile_dir, snapshot).await;
        });
    }
    Ok(())
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    ui_tx: mpsc::UnboundedSender<McpHostRequest>,
    session_token: String,
    profile_dir: std::path::PathBuf,
    snapshot: Arc<RwLock<McpSessionSnapshot>>,
) -> Result<(), String> {
    let (reader, mut writer) = stream.into_split();
    let mut buf = BufReader::new(reader);
    let mut line = String::new();
    buf.read_line(&mut line)
        .await
        .map_err(|e| e.to_string())?;
    let req: McpIpcRequest = decode_line(&line).map_err(|e| e.to_string())?;

    let response = match req {
        McpIpcRequest::Ping { token } => {
            if token == session_token {
                McpIpcResponse::success(serde_json::json!({ "pong": true }))
            } else {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            }
        }
        McpIpcRequest::Session { token } => {
            if token != session_token {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            } else {
                let snap = snapshot.read().map_err(|_| "session snapshot poisoned")?;
                if let (Some(address), Some(chain_id), Some(network_id)) =
                    (&snap.address, snap.chain_id, &snap.network_id)
                {
                    McpIpcResponse::success(serde_json::json!({
                        "address": address,
                        "chain_id": chain_id,
                        "network_id": network_id,
                    }))
                } else {
                    McpIpcResponse::failure("wallet_locked", "wallet is locked")
                }
            }
        }
        McpIpcRequest::ProposalStatus { token, proposal_id } => {
            if token != session_token {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            } else {
                let queue = ProposalQueue::new(&profile_dir);
                match queue.get_pending(&proposal_id, session_token.as_bytes()) {
                    Ok(_) => McpIpcResponse::success(serde_json::json!({
                        "proposal_id": proposal_id,
                        "status": "pending_user",
                    })),
                    Err(_) => McpIpcResponse::success(serde_json::json!({
                        "proposal_id": proposal_id,
                        "status": "unknown",
                    })),
                }
            }
        }
        McpIpcRequest::Propose {
            token,
            source,
            proposal,
        } => {
            if token != session_token {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            } else {
                let (reply_tx, reply_rx) = oneshot::channel();
                if ui_tx
                    .send(McpHostRequest::Propose {
                        proposal,
                        source,
                        reply: Some(reply_tx),
                    })
                    .is_err()
                {
                    McpIpcResponse::failure("tui_offline", "wallet UI is closed")
                } else {
                    match reply_rx.await {
                        Ok(Ok(tx_hash)) => McpIpcResponse::success(serde_json::json!({
                            "status": "approved",
                            "tx_hash": tx_hash,
                        })),
                        Ok(Err(e)) => McpIpcResponse::failure("user_rejected", e.to_string()),
                        Err(_) => McpIpcResponse::failure("tui_offline", "approval channel closed"),
                    }
                }
            }
        }
    };

    let out = encode_line(&response).map_err(|e| e.to_string())?;
    writer.write_all(out.as_bytes()).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}
