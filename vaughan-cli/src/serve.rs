//! Headless wallet daemon (`vaughan serve`) — v2 signing boundary.
//!
//! Unlocks a profile vault, publishes an MCP session token, and serves the
//! loopback control plane. Sentient profiles auto-exec; default queues for a
//! later TUI approve (or use sentient for agent autonomy).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use secrecy::SecretString;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use vaughan_agent::paths::profile_dir;
use vaughan_core::core::mcp_ipc::{decode_line, encode_line, McpIpcRequest, McpIpcResponse};
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
            OperatingMode::DegenTrader
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
            "WARNING: sentient/degen auto-signs over loopback IPC while unlocked — \
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
    while !stop.load(Ordering::SeqCst) {
        let accept =
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept()).await;
        let Ok(Ok((stream, _))) = accept else {
            continue;
        };
        let wallet = wallet.clone();
        let session = session.clone();
        let profile_dir = profile_dir.clone();
        let profile_name = profile.clone();
        let handle = handle.clone();
        tokio::spawn(async move {
            let _ = handle_conn(stream, wallet, session, profile_dir, profile_name, handle).await;
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

async fn handle_conn(
    stream: tokio::net::TcpStream,
    wallet: Arc<Mutex<WalletState>>,
    session_token: String,
    profile_dir: PathBuf,
    profile_name: String,
    handle: Handle,
) -> Result<(), String> {
    let (reader, mut writer) = stream.into_split();
    let mut buf = BufReader::new(reader);
    let mut line = String::new();
    buf.read_line(&mut line).await.map_err(|e| e.to_string())?;
    let req: McpIpcRequest = decode_line(&line).map_err(|e| e.to_string())?;

    let response = match req {
        McpIpcRequest::Ping { token } => {
            if token == session_token {
                McpIpcResponse::success(serde_json::json!({ "pong": true, "serve": true }))
            } else {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            }
        }
        McpIpcRequest::Session { token } => {
            if token != session_token {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            } else {
                let w = wallet.lock().map_err(|_| "wallet lock poisoned")?;
                let addr = w.active_address().map_err(|e| e.to_string())?;
                let net = w.networks().active();
                McpIpcResponse::success(serde_json::json!({
                    "address": addr,
                    "chain_id": net.chain_id,
                    "network_id": net.id,
                    "serve": true,
                }))
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
                match execute_propose(
                    &wallet,
                    &handle,
                    &profile_dir,
                    &profile_name,
                    &source,
                    *proposal,
                    &session_token,
                ) {
                    Ok(ExecOutcome::Approved { tx_hash }) => {
                        McpIpcResponse::success(serde_json::json!({
                            "status": "approved",
                            "tx_hash": tx_hash,
                            "serve": true,
                        }))
                    }
                    Ok(ExecOutcome::Queued { proposal_id }) => {
                        McpIpcResponse::success(serde_json::json!({
                            "status": "pending_user",
                            "proposal_id": proposal_id,
                            "serve": true,
                            "message": "Queued — open Vaughan TUI on this profile to approve",
                        }))
                    }
                    Err(e) => McpIpcResponse::failure("exec_failed", e),
                }
            }
        }
        McpIpcRequest::StealthUri { token } => {
            if token != session_token {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            } else {
                let w = wallet.lock().map_err(|_| "wallet lock poisoned")?;
                match w.stealth_uri() {
                    Ok(uri) => McpIpcResponse::success(serde_json::json!({ "uri": uri })),
                    Err(e) => McpIpcResponse::failure("stealth_error", e.to_string()),
                }
            }
        }
        McpIpcRequest::StealthScan { token } => {
            if token != session_token {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            } else {
                let notes = {
                    let w = wallet.lock().map_err(|_| "wallet lock poisoned")?;
                    handle
                        .block_on(w.scan_stealth_notes())
                        .map_err(|e| e.to_string())
                };
                match notes {
                    Ok(notes) => {
                        let rows: Vec<_> = notes
                            .iter()
                            .map(|n| {
                                serde_json::json!({
                                    "stealth_address": format!("{:#x}", n.announcement.stealth_address),
                                    "balance_wei": n.balance_wei.to_string(),
                                    "balance": n.balance_formatted,
                                    "view_tag": n.announcement.view_tag,
                                })
                            })
                            .collect();
                        McpIpcResponse::success(serde_json::json!({
                            "notes": rows,
                            "count": rows.len(),
                        }))
                    }
                    Err(e) => McpIpcResponse::failure("stealth_error", e),
                }
            }
        }
        McpIpcRequest::StealthSweep {
            token,
            stealth_address,
        } => {
            if token != session_token {
                McpIpcResponse::failure("unauthorized", "invalid session token")
            } else if !mcp_auto_exec_enabled(&profile_name) {
                McpIpcResponse::failure(
                    "pending_user",
                    "stealth sweep on adviser profile needs unlocked TUI approval card",
                )
            } else {
                let outcome = (|| -> Result<String, String> {
                    let w = wallet
                        .lock()
                        .map_err(|_| "wallet lock poisoned".to_string())?;
                    let notes = handle
                        .block_on(w.scan_stealth_notes())
                        .map_err(|e| e.to_string())?;
                    let note = notes
                        .into_iter()
                        .find(|n| {
                            format!("{:#x}", n.announcement.stealth_address)
                                .eq_ignore_ascii_case(&stealth_address)
                        })
                        .ok_or_else(|| format!("no unswept stealth note for {stealth_address}"))?;
                    handle
                        .block_on(w.sweep_stealth_note(&note))
                        .map(|h| h.to_string())
                        .map_err(|e| e.to_string())
                })();
                match outcome {
                    Ok(tx_hash) => McpIpcResponse::success(serde_json::json!({
                        "status": "approved",
                        "tx_hash": tx_hash,
                        "serve": true,
                    })),
                    Err(e) => McpIpcResponse::failure("stealth_error", e),
                }
            }
        }
    };

    let out = encode_line(&response).map_err(|e| e.to_string())?;
    writer
        .write_all(out.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

enum ExecOutcome {
    Approved { tx_hash: String },
    Queued { proposal_id: String },
}

fn execute_propose(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    profile_dir: &std::path::Path,
    profile_name: &str,
    source: &str,
    proposal: TxProposal,
    session_token: &str,
) -> Result<ExecOutcome, String> {
    {
        let w = wallet.lock().map_err(|_| "wallet lock poisoned")?;
        let net = w.networks().active();
        guard_mainnet_write(proposal.chain_id, net.is_testnet).map_err(|e| e.to_string())?;
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
        let w = wallet.lock().map_err(|_| "wallet lock poisoned")?;
        let hash =
            sentient_mcp::auto_exec_mcp_proposal(&w, handle, &kind).map_err(|e| e.to_string())?;
        return Ok(ExecOutcome::Approved { tx_hash: hash });
    }

    let queue = ProposalQueue::new(profile_dir);
    queue
        .enqueue(proposal, source, session_token.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(ExecOutcome::Queued { proposal_id })
}
