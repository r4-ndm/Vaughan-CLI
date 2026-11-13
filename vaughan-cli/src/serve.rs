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
use vaughan_core::core::signer_client::SignerClient;
use vaughan_core::core::{
    is_sentient_profile, reject_deferred_sentient_profile, OperatingMode, StateManager, WalletState,
};
use vaughan_tui::sentient_mcp::{self, mcp_auto_exec_enabled};

/// Run until Ctrl-C. Requires `--password-env` for non-interactive unlock on adviser profiles.
///
/// For sentient profiles, acts as a thin client to `vaughan-signerd`: no TCP listener,
/// no `mcp.session` token file, no local signing or wallet unlock.
pub async fn run_serve(profile: String, password: SecretString) -> anyhow::Result<()> {
    reject_deferred_sentient_profile(&profile)
        .map_err(|e| anyhow::anyhow!("{}", e.user_message()))?;

    if is_sentient_profile(&profile) {
        let client = SignerClient::default();
        let status = client.status().await.map_err(|e| {
            anyhow::anyhow!(
                "signer daemon unreachable at {} ({e}) — ensure vaughan-signerd is active",
                client.socket_path().display()
            )
        })?;

        eprintln!(
            "vaughan serve: profile={profile} connected to signerd at {}",
            client.socket_path().display()
        );
        if let Some(ref addr) = status.address {
            eprintln!("active address: {addr}");
        }
        if let Some(cid) = status.chain_id {
            eprintln!("chain id: {cid}");
        }
        eprintln!("thin client active, monitoring signerd. Ctrl-C to stop");

        let stop = Arc::new(AtomicBool::new(false));
        let stop_c = stop.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            stop_c.store(true, Ordering::SeqCst);
        });

        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        while !stop.load(Ordering::SeqCst) {
            ticker.tick().await;
            if !client.ping().await && !stop.load(Ordering::SeqCst) {
                eprintln!(
                    "warning: vaughan-signerd ping failed at {}",
                    client.socket_path().display()
                );
            }
        }
        eprintln!("vaughan serve: stopped");
        return Ok(());
    }

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
    // Bound concurrent connections and cap each connection's lifetime so a
    // local process cannot exhaust fds/memory by trickling bytes.
    let permits = Arc::new(tokio::sync::Semaphore::new(32));
    while !stop.load(Ordering::SeqCst) {
        let accept =
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept()).await;
        let Ok(Ok((stream, _))) = accept else {
            continue;
        };
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            eprintln!("vaughan serve: connection cap reached, dropping");
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
            let _permit = permit;
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(180),
                handle_ipc_connection(stream, token, dir, backend),
            )
            .await;
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
        let account_index = w.active_account_index().map_err(|e| e.to_string())?;
        let account_label = w.active_account_label().map_err(|e| e.to_string())?;
        Ok(McpSessionData {
            address: addr.to_string(),
            chain_id: net.chain_id,
            network_id: net.id.clone(),
            account_index,
            account_label: account_label.to_string(),
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

    async fn stealth_sweep(&self, _stealth_address: &str) -> Result<String, String> {
        Err(
            "tui_required: adviser serve cannot show a sweep card — unlock Vaughan TUI on \
             this profile"
                .into(),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_propose(
    wallet: &Arc<Mutex<WalletState>>,
    _handle: &Handle,
    profile_dir: &Path,
    profile_name: &str,
    source: &str,
    proposal: TxProposal,
    session_token: &str,
    _breaker: Option<&vaughan_agent::CircuitBreaker>,
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

    if is_sentient_profile(profile_name) {
        return Err(
            "sentient_local_signing_disabled: sentient profile must use vaughan-signerd socket"
                .into(),
        );
    }

    let queue = ProposalQueue::new(profile_dir);
    let secret = session_token.as_bytes().to_vec();
    queue
        .enqueue(proposal, source, &secret)
        .map_err(|e| e.to_string())?;
    Ok(McpProposeOutcome::Queued {
        proposal_id,
        message: "Queued — open Vaughan TUI on this profile to approve".into(),
    })
}

/// Validate the `VAUGHAN_SIGNER_STORE` environment variable for sentient serve.
///
/// Refuses to start without `VAUGHAN_SIGNER_STORE`. The directory must exist and have mode 0700.
#[allow(dead_code)]
pub fn validate_signer_store_env() -> anyhow::Result<PathBuf> {
    let raw = std::env::var("VAUGHAN_SIGNER_STORE").map_err(|_| {
        anyhow::anyhow!(
            "VAUGHAN_SIGNER_STORE is required for sentient serve — signer store is unconfigured.\n\
             Export the signer store path and create the directory with mode 0700:\n    \
                 export VAUGHAN_SIGNER_STORE=/var/lib/vaughan/sentient\n    \
                 mkdir -p /var/lib/vaughan/sentient && chmod 0700 /var/lib/vaughan/sentient"
        )
    })?;
    let path = PathBuf::from(raw.trim());
    if raw.trim().is_empty() || !path.exists() || !path.is_dir() {
        anyhow::bail!(
            "VAUGHAN_SIGNER_STORE directory `{}` does not exist.\n\
             Create it with mode 0700:\n    \
                 mkdir -p {} && chmod 0700 {}",
            path.display(),
            path.display(),
            path.display(),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(&path).map_err(|e| {
            anyhow::anyhow!(
                "failed to read VAUGHAN_SIGNER_STORE metadata {}: {e}",
                path.display()
            )
        })?;
        let mode = meta.mode() & 0o777;
        if mode != 0o700 {
            anyhow::bail!(
                "VAUGHAN_SIGNER_STORE `{}` has invalid permissions: {:04o} (expected 0700).\n\
                 Fix permissions with:\n    \
                     chmod 0700 {}",
                path.display(),
                mode,
                path.display(),
            );
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_missing_signer_store_env_fails() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("VAUGHAN_SIGNER_STORE");
        let err = validate_signer_store_env().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("VAUGHAN_SIGNER_STORE is required"));
        assert!(msg.contains("export VAUGHAN_SIGNER_STORE="));
    }

    #[test]
    fn test_nonexistent_signer_store_fails() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "VAUGHAN_SIGNER_STORE",
            "/tmp/nonexistent_signer_store_path_12345",
        );
        let err = validate_signer_store_env().unwrap_err();
        assert!(err.to_string().contains("does not exist"));
        std::env::remove_var("VAUGHAN_SIGNER_STORE");
    }

    #[cfg(unix)]
    #[test]
    fn test_signer_store_mode_0755_fails_mode_0700_succeeds() {
        let _lock = ENV_LOCK.lock().unwrap();
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let store = dir.path().join("sentient_store");
        std::fs::create_dir(&store).unwrap();

        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("VAUGHAN_SIGNER_STORE", &store);
        let err = validate_signer_store_env().unwrap_err();
        assert!(err.to_string().contains("0755") && err.to_string().contains("0700"));

        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate_signer_store_env().is_ok());
        std::env::remove_var("VAUGHAN_SIGNER_STORE");
    }
}
