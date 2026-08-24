//! MCP control-plane listener — loopback TCP for external agent proposals.
//!
//! Mirrors the EIP-1193 provider pattern: the listener runs on a tokio task and
//! forwards propose requests to the UI thread over an MPSC channel.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use vaughan_core::chains::evm::networks::get_network_by_id;
use vaughan_core::core::mcp_host::{
    handle_ipc_connection, McpHostBackend, McpProposeOutcome, McpSessionData,
};
use vaughan_core::core::proposal::{
    guard_mainnet_write, mcp_control_port, McpSessionToken, ProposalQueue, TxProposal,
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
    StealthUri {
        reply: oneshot::Sender<Result<String, ProviderError>>,
    },
    StealthScan {
        reply: oneshot::Sender<Result<serde_json::Value, ProviderError>>,
    },
    StealthSweep {
        stealth_address: String,
        reply: oneshot::Sender<Result<String, ProviderError>>,
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
            if queue.get_pending(&id, secret.as_bytes()).is_err() {
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

struct TuiMcpBackend {
    ui_tx: mpsc::UnboundedSender<McpHostRequest>,
    snapshot: Arc<RwLock<McpSessionSnapshot>>,
}

#[async_trait]
impl McpHostBackend for TuiMcpBackend {
    fn host_tag(&self) -> Option<&'static str> {
        Some("tui")
    }

    async fn session(&self) -> Result<McpSessionData, String> {
        let snap = self
            .snapshot
            .read()
            .map_err(|_| "session snapshot poisoned")?;
        match (&snap.address, snap.chain_id, &snap.network_id) {
            (Some(address), Some(chain_id), Some(network_id)) => Ok(McpSessionData {
                address: address.clone(),
                chain_id,
                network_id: network_id.clone(),
            }),
            _ => Err("wallet is locked".into()),
        }
    }

    async fn propose(
        &self,
        source: &str,
        proposal: TxProposal,
    ) -> Result<McpProposeOutcome, String> {
        let (chain_id, network_id) = {
            let snap = self
                .snapshot
                .read()
                .map_err(|_| "session snapshot poisoned".to_string())?;
            match (snap.chain_id, snap.network_id.clone()) {
                (Some(chain_id), Some(network_id)) => (chain_id, network_id),
                _ => return Err("wallet is locked".into()),
            }
        };
        let net = get_network_by_id(&network_id)
            .ok_or_else(|| format!("unknown network: {network_id}"))?;
        guard_mainnet_write(proposal.chain_id, net.is_testnet).map_err(|e| e.to_string())?;
        if proposal.chain_id != 0 && proposal.chain_id != chain_id {
            return Err(format!(
                "network_mismatch: proposal chain_id {} != active {chain_id}",
                proposal.chain_id
            ));
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        self.ui_tx
            .send(McpHostRequest::Propose {
                proposal: Box::new(proposal),
                source: source.to_string(),
                reply: Some(reply_tx),
            })
            .map_err(|_| "wallet UI is closed".to_string())?;
        match reply_rx.await {
            Ok(Ok(tx_hash)) => Ok(McpProposeOutcome::Approved { tx_hash }),
            Ok(Err(e)) => Ok(McpProposeOutcome::Rejected {
                reason: e.to_string(),
            }),
            Err(_) => Err("approval channel closed".into()),
        }
    }

    async fn stealth_uri(&self) -> Result<String, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.ui_tx
            .send(McpHostRequest::StealthUri { reply: reply_tx })
            .map_err(|_| "wallet UI is closed".to_string())?;
        match reply_rx.await {
            Ok(Ok(uri)) => Ok(uri),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("channel closed".into()),
        }
    }

    async fn stealth_scan(&self) -> Result<serde_json::Value, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.ui_tx
            .send(McpHostRequest::StealthScan { reply: reply_tx })
            .map_err(|_| "wallet UI is closed".to_string())?;
        match reply_rx.await {
            Ok(Ok(data)) => Ok(data),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("channel closed".into()),
        }
    }

    async fn stealth_sweep(&self, stealth_address: &str) -> Result<String, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.ui_tx
            .send(McpHostRequest::StealthSweep {
                stealth_address: stealth_address.to_string(),
                reply: reply_tx,
            })
            .map_err(|_| "wallet UI is closed".to_string())?;
        match reply_rx.await {
            Ok(Ok(tx_hash)) => Ok(tx_hash),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("channel closed".into()),
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
        let accept =
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept()).await;
        let Ok(Ok((stream, _))) = accept else {
            continue;
        };
        let backend = TuiMcpBackend {
            ui_tx: ui_tx.clone(),
            snapshot: snapshot.clone(),
        };
        let token = session_token.clone();
        let dir = profile_dir.clone();
        tokio::spawn(async move {
            let _ = handle_ipc_connection(stream, token, dir, backend).await;
        });
    }
    Ok(())
}
