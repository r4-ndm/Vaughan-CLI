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

/// Whether the loopback MCP control plane (agents / Cursor) is reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpListenerState {
    #[default]
    Off,
    /// Background task is binding or waiting for first accept.
    Starting,
    /// Listening on loopback (default port 8746).
    Active,
    /// Bind failed (e.g. port in use) — agents cannot attach.
    Unavailable,
}

/// Manages MCP session token + background listener lifecycle.
pub struct McpService {
    profile_dir: std::path::PathBuf,
    session: Option<McpSessionToken>,
    listener: ListenerState,
    requests: mpsc::Sender<McpHostRequest>,
    surfaced: HashSet<String>,
    snapshot: Arc<RwLock<McpSessionSnapshot>>,
    /// Set when the control-plane port is bound (synchronously, before the
    /// token file is written).
    listener_bound: Arc<AtomicBool>,
    /// Set when bind fails (port busy, permission, …).
    listener_bind_failed: Arc<AtomicBool>,
    /// Earliest time a failed bind may be retried (backs off the tick loop).
    retry_after: Option<std::time::Instant>,
}

/// Bound on concurrent control-plane connections (slowloris / fd exhaustion).
const MCP_MAX_CONNECTIONS: usize = 32;
/// Total per-connection lifetime cap — must exceed the 60s approval TTL.
const MCP_CONNECTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
/// Delay before retrying a failed control-plane bind.
const BIND_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

impl McpService {
    pub fn new(profile_dir: &Path, requests: mpsc::Sender<McpHostRequest>) -> Self {
        Self {
            profile_dir: profile_dir.to_path_buf(),
            session: None,
            listener: ListenerState::new(),
            requests,
            surfaced: HashSet::new(),
            snapshot: Arc::new(RwLock::new(McpSessionSnapshot::default())),
            listener_bound: Arc::new(AtomicBool::new(false)),
            listener_bind_failed: Arc::new(AtomicBool::new(false)),
            retry_after: None,
        }
    }

    /// Chrome / status strip: is external MCP (Cursor, Claude, …) able to attach?
    pub fn listener_state(&self) -> McpListenerState {
        if self.session.is_none() {
            return McpListenerState::Off;
        }
        if self.listener_bound.load(Ordering::Relaxed) {
            return McpListenerState::Active;
        }
        if self.listener_bind_failed.load(Ordering::Relaxed) {
            return McpListenerState::Unavailable;
        }
        McpListenerState::Starting
    }

    /// Start or restart the loopback listener while the wallet stays unlocked.
    pub fn ensure_listener(&mut self, runtime: &tokio::runtime::Handle) {
        if self.listener_bound.load(Ordering::Relaxed) {
            return;
        }
        if self.session.is_some() && !self.listener_bind_failed.load(Ordering::Relaxed) {
            return;
        }
        if self.session.is_some() {
            // Bind failed: tear down, then back off — retrying every tick
            // would churn the token file and spam the log.
            self.stop_listener_task();
            self.session = None;
            self.listener_bind_failed.store(false, Ordering::SeqCst);
            let _ = McpSessionToken::invalidate(&self.profile_dir);
            self.retry_after = Some(std::time::Instant::now() + BIND_RETRY_BACKOFF);
            return;
        }
        if let Some(retry_at) = self.retry_after {
            if std::time::Instant::now() < retry_at {
                return;
            }
            self.retry_after = None;
        }
        self.start_listener(runtime);
    }

    /// Update the session snapshot the loopback listener serves via `Session` IPC.
    pub fn update_session(&self, snapshot: McpSessionSnapshot) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = snapshot;
        }
    }

    /// Start session token + loopback listener when wallet unlocks.
    pub fn on_unlock(&mut self, runtime: &tokio::runtime::Handle) {
        self.ensure_listener(runtime);
    }

    fn start_listener(&mut self, runtime: &tokio::runtime::Handle) {
        if self.session.is_some() {
            return;
        }
        self.listener_bound.store(false, Ordering::SeqCst);
        self.listener_bind_failed.store(false, Ordering::SeqCst);

        // Bind synchronously FIRST: the token file is only written once the
        // port is actually ours. A live token with no listener would let a
        // port-squatting process impersonate the control plane toward agents.
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, mcp_control_port()));
        let std_listener = match std::net::TcpListener::bind(addr) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("mcp listener bind {addr} failed: {e}");
                self.listener_bind_failed.store(true, Ordering::SeqCst);
                self.retry_after = Some(std::time::Instant::now() + BIND_RETRY_BACKOFF);
                return;
            }
        };
        if std_listener.set_nonblocking(true).is_err() {
            self.listener_bind_failed.store(true, Ordering::SeqCst);
            self.retry_after = Some(std::time::Instant::now() + BIND_RETRY_BACKOFF);
            return;
        }

        let token = McpSessionToken::generate();
        if token.write(&self.profile_dir).is_err() {
            self.listener_bind_failed.store(true, Ordering::SeqCst);
            self.retry_after = Some(std::time::Instant::now() + BIND_RETRY_BACKOFF);
            return;
        }
        self.listener_bound.store(true, Ordering::SeqCst);
        self.listener.stop.store(false, Ordering::SeqCst);
        let stop = self.listener.stop.clone();
        let tx = self.requests.clone();
        let session = token.as_str().to_string();
        let profile_dir = self.profile_dir.clone();
        let snapshot = self.snapshot.clone();
        let handle = runtime.spawn(async move {
            match TcpListener::from_std(std_listener) {
                Ok(listener) => {
                    run_listener(listener, stop, tx, session, profile_dir, snapshot).await
                }
                Err(e) => Err(format!("from_std: {e}")),
            }
            .unwrap_or_else(|e| tracing::warn!("mcp listener stopped: {e}"));
        });
        self.listener.handle = Some(handle);
        self.session = Some(token);
    }

    fn stop_listener_task(&mut self) {
        self.listener.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.listener.handle.take() {
            handle.abort();
        }
        self.listener_bound.store(false, Ordering::SeqCst);
    }

    /// Stop listener and invalidate session token when wallet locks.
    pub fn on_lock(&mut self) {
        self.stop_listener_task();
        self.session = None;
        self.listener_bind_failed.store(false, Ordering::SeqCst);
        self.retry_after = None;
        self.surfaced.clear();
        self.update_session(McpSessionSnapshot::default());
        let _ = McpSessionToken::invalidate(&self.profile_dir);
    }

    /// Rebind the control plane to a different profile dir (unlock-screen
    /// profile switch). Stops any listener and invalidates the old session
    /// token so no stale control plane survives the switch.
    pub fn set_profile_dir(&mut self, dir: &std::path::Path) {
        if self.profile_dir == dir {
            return;
        }
        self.on_lock();
        self.profile_dir = dir.to_path_buf();
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
            if let Err(e) = self.requests.try_send(McpHostRequest::Propose {
                proposal: Box::new(queued.proposal),
                source: queued.source,
                reply: None,
            }) {
                tracing::warn!(target: "vaughan_tui::mcp", "UI queue full, dropping queued proposal: {e}");
            }
            break;
        }
    }
}

struct TuiMcpBackend {
    ui_tx: mpsc::Sender<McpHostRequest>,
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
        guard_mainnet_write(net.is_testnet).map_err(|e| e.to_string())?;
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
            .await
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
            .await
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
            .await
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
            .await
            .map_err(|_| "wallet UI is closed".to_string())?;
        match reply_rx.await {
            Ok(Ok(tx_hash)) => Ok(tx_hash),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("channel closed".into()),
        }
    }
}

async fn run_listener(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    ui_tx: mpsc::Sender<McpHostRequest>,
    session_token: String,
    profile_dir: std::path::PathBuf,
    snapshot: Arc<RwLock<McpSessionSnapshot>>,
) -> Result<(), String> {
    // Bound concurrent connections and cap each connection's lifetime so a
    // local process cannot exhaust fds/memory by trickling bytes.
    let permits = Arc::new(tokio::sync::Semaphore::new(MCP_MAX_CONNECTIONS));
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let accept =
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept()).await;
        let Ok(Ok((stream, _))) = accept else {
            continue;
        };
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            tracing::warn!(target: "vaughan_tui::mcp", "mcp connection cap reached, dropping");
            continue;
        };
        let backend = TuiMcpBackend {
            ui_tx: ui_tx.clone(),
            snapshot: snapshot.clone(),
        };
        let token = session_token.clone();
        let dir = profile_dir.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = tokio::time::timeout(
                MCP_CONNECTION_TIMEOUT,
                handle_ipc_connection(stream, token, dir, backend),
            )
            .await;
        });
    }
    Ok(())
}
