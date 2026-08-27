//! Headless wallet daemon (`vaughan serve`) — v2 signing boundary.
//!
//! Unlocks a profile vault, publishes an MCP session token, and serves the
//! loopback control plane. Sentient profiles auto-exec; default queues for a
//! later TUI approve (or use sentient for agent autonomy).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use secrecy::SecretString;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use vaughan_agent::paths::profile_dir;
use vaughan_core::core::mcp_host::{
    handle_ipc_connection, McpHostBackend, McpProposeOutcome, McpSessionData,
};
use vaughan_core::core::proposal::{
    guard_mainnet_write, mcp_control_port, McpSessionToken, ProposalQueue, TxProposal,
};
use vaughan_core::core::{is_sentient_profile, OperatingMode, StateManager, WalletState};
use vaughan_tui::provider::ApprovalKind;
use vaughan_tui::sentient_mcp::{self, mcp_auto_exec_enabled};

/// Run until Ctrl-C. Requires `--password-env` for non-interactive unlock.
pub async fn run_serve(profile: String, password: SecretString) -> anyhow::Result<()> {
    let path = StateManager::profile_path(&profile)?;
    let mut wallet = WalletState::load_with_session(
        path.clone(),
        if is_sentient_profile(&profile) {
            OperatingMode::SentientTrader
        } else {
            OperatingMode::AiAssisted
        },
        profile.clone(),
    )?;
    if !wallet.is_initialized() {
        anyhow::bail!(
            "no wallet at {} — run `vaughan --profile {profile} create` first",
            path.display()
        );
    }
    wallet.unlock(&password)?;
    let profile_dir = profile_dir(wallet.path());
    let token = McpSessionToken::generate();
    token.write(&profile_dir)?;

    let stop = Arc::new(AtomicBool::new(false));
    let wallet = Arc::new(Mutex::new(wallet));
    let handle = Handle::current();
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, mcp_control_port()));
    let listener = TcpListener::bind(addr).await?;
    eprintln!(
        "vaughan serve: profile={profile} listening on {addr} (sentient_auto={})",
        mcp_auto_exec_enabled(&profile)
    );
    eprintln!("session token written under {}", profile_dir.display());
    if mcp_auto_exec_enabled(&profile) {
        eprintln!(
            "WARNING: sentient profile auto-signs over loopback IPC while unlocked — \
             treat this host as a hot wallet; same-user processes with the session token can spend"
        );
    }
    eprintln!("Ctrl-C to stop");

    let stop_c = stop.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        stop_c.store(true, Ordering::SeqCst);
    });

    let session = token.as_str().to_string();
    // Session-scoped circuit breaker: shared across connections so cumulative
    // gas and consecutive-error tripwires accumulate for the whole serve run.
    let breaker = if mcp_auto_exec_enabled(&profile) {
        let w = wallet
            .lock()
            .map_err(|_| anyhow::anyhow!("wallet lock poisoned"))?;
        Some(
            sentient_mcp::new_session_breaker(&w)
                .map_err(|e| anyhow::anyhow!("sentient policy: {e}"))?,
        )
    } else {
        None
    };
    while !stop.load(Ordering::SeqCst) {
        let accept =
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept()).await;
        let Ok(Ok((stream, _))) = accept else {
            continue;
        };
        let backend = ServeMcpBackend {
            wallet: wallet.clone(),
            handle: handle.clone(),
            profile_dir: profile_dir.clone(),
            profile_name: profile.clone(),
            session_token: session.clone(),
            breaker: breaker.clone(),
        };
        let token = session.clone();
        let dir = profile_dir.clone();
        tokio::spawn(async move {
            let _ = handle_ipc_connection(stream, token, dir, backend).await;
        });
    }

    let _ = McpSessionToken::invalidate(&profile_dir);
    Ok(())
}

pub fn password_from_env(password_env: Option<&str>) -> anyhow::Result<SecretString> {
    let var = password_env.ok_or_else(|| {
        anyhow::anyhow!("vaughan serve requires --password-env NAME (non-interactive unlock)")
    })?;
    let value = std::env::var(var)
        .map_err(|_| anyhow::anyhow!("environment variable `{var}` is not set"))?;
    Ok(SecretString::from(value))
}

struct ServeMcpBackend {
    wallet: Arc<Mutex<WalletState>>,
    handle: Handle,
    profile_dir: PathBuf,
    profile_name: String,
    session_token: String,
    breaker: Option<vaughan_agent::CircuitBreaker>,
}

#[async_trait]
impl McpHostBackend for ServeMcpBackend {
    fn host_tag(&self) -> Option<&'static str> {
        Some("serve")
    }

    async fn session(&self) -> Result<McpSessionData, String> {
        let w = self.wallet.lock().map_err(|_| "wallet lock poisoned")?;
        let addr = w.active_address().map_err(|e| e.to_string())?;
        let net = w.networks().active();
        Ok(McpSessionData {
            address: addr.to_string(),
            chain_id: net.chain_id,
            network_id: net.id.clone(),
        })
    }

    async fn propose(
        &self,
        source: &str,
        proposal: TxProposal,
    ) -> Result<McpProposeOutcome, String> {
        // block_in_place: execute_propose drives Handle::block_on internally,
        // which panics if called directly on a runtime worker thread.
        tokio::task::block_in_place(|| {
            execute_propose(
                &self.wallet,
                &self.handle,
                &self.profile_dir,
                &self.profile_name,
                source,
                proposal,
                &self.session_token,
                self.breaker.as_ref(),
            )
        })
    }

    async fn stealth_uri(&self) -> Result<String, String> {
        let w = self.wallet.lock().map_err(|_| "wallet lock poisoned")?;
        w.stealth_uri().map_err(|e| e.to_string())
    }

    async fn stealth_scan(&self) -> Result<Value, String> {
        let notes = tokio::task::block_in_place(|| {
            let w = self.wallet.lock().map_err(|_| "wallet lock poisoned")?;
            self.handle
                .block_on(w.scan_stealth_notes())
                .map_err(|e| e.to_string())
        })?;
        let rows: Vec<_> = notes
            .iter()
            .map(|n| {
                json!({
                    "stealth_address": format!("{:#x}", n.announcement.stealth_address),
                    "balance_wei": n.balance_wei.to_string(),
                    "balance": n.balance_formatted,
                    "view_tag": n.announcement.view_tag,
                })
            })
            .collect();
        Ok(json!({ "notes": rows, "count": rows.len() }))
    }

    async fn stealth_sweep(&self, stealth_address: &str) -> Result<String, String> {
        if !mcp_auto_exec_enabled(&self.profile_name) {
            return Err(
                "tui_required: adviser serve cannot show a sweep card — unlock Vaughan TUI on \
                 this profile (or use --profile sentient for headless auto-sweep)"
                    .into(),
            );
        }
        tokio::task::block_in_place(|| {
            let w = self
                .wallet
                .lock()
                .map_err(|_| "wallet lock poisoned".to_string())?;
            let notes = self
                .handle
                .block_on(w.scan_stealth_notes())
                .map_err(|e| e.to_string())?;
            let note = notes
                .into_iter()
                .find(|n| {
                    format!("{:#x}", n.announcement.stealth_address)
                        .eq_ignore_ascii_case(stealth_address)
                })
                .ok_or_else(|| format!("no unswept stealth note for {stealth_address}"))?;
            self.handle
                .block_on(w.sweep_stealth_note(&note))
                .map(|h| h.to_string())
                .map_err(|e| e.to_string())
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_propose(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    profile_dir: &Path,
    profile_name: &str,
    source: &str,
    proposal: TxProposal,
    session_token: &str,
    breaker: Option<&vaughan_agent::CircuitBreaker>,
) -> Result<McpProposeOutcome, String> {
    {
        let w = wallet.lock().map_err(|_| "wallet lock poisoned")?;
        let net = w.networks().active();
        guard_mainnet_write(net.is_testnet).map_err(|e| e.to_string())?;
        if proposal.chain_id != 0 && proposal.chain_id != net.chain_id {
            return Err(format!(
                "network_mismatch: proposal chain_id {} != active {}",
                proposal.chain_id, net.chain_id
            ));
        }
    }

    let proposal_id = proposal.proposal_id.clone();
    let kind = ApprovalKind::McpProposal {
        proposal_id: proposal_id.clone(),
        source: source.to_string(),
        proposal: Box::new(proposal.clone()),
    };

    if mcp_auto_exec_enabled(profile_name) {
        let breaker = breaker.ok_or_else(|| "sentient circuit breaker unavailable".to_string())?;
        let w = wallet.lock().map_err(|_| "wallet lock poisoned")?;
        let hash = sentient_mcp::auto_exec_mcp_proposal(&w, handle, breaker, &kind)
            .map_err(|e| e.to_string())?;
        return Ok(McpProposeOutcome::Approved { tx_hash: hash });
    }

    let queue = ProposalQueue::new(profile_dir);
    queue
        .enqueue(proposal, source, session_token.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(McpProposeOutcome::Queued {
        proposal_id,
        message: "Queued — open Vaughan TUI on this profile to approve".into(),
    })
}
