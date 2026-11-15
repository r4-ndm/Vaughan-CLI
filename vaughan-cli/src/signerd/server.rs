//! Unix domain socket accept loop for `vaughan signerd`.
//!
//! Single purpose: unlock wallet state, bind socket with `0660` permissions and
//! optional `vaughan` group ownership, verify `SO_PEERCRED`, and dispatch lines
//! to [`Signer`]. Must NOT contain policy logic, spend checks, or calldata ops.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use alloy::primitives::Address;
use secrecy::SecretString;
use tokio::io::AsyncWriteExt;
use vaughan_core::core::signer_client::SignerResponse;
use vaughan_core::core::{OperatingMode, WalletState};
use zeroize::Zeroize;

use super::{
    balance, broadcaster, check_legacy_policy_absent, find_vaughan_gid, is_peer_uid_allowed,
    quoter, Signer,
};

/// Run the daemon loop serving Unix domain socket connections.
pub async fn run_signerd(
    store: PathBuf,
    socket: PathBuf,
    allow_uid: Vec<u32>,
    password: SecretString,
    old_profile_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    check_legacy_policy_absent(&store, old_profile_dir.as_deref())?;

    #[cfg(unix)]
    let allow_uid = if allow_uid.is_empty() {
        vec![rustix::process::getuid().as_raw()]
    } else {
        allow_uid
    };

    let wallet_path = store.join("wallet.json");
    if !wallet_path.is_file() {
        anyhow::bail!(
            "no wallet at {} — create or migrate the wallet into the store first",
            wallet_path.display()
        );
    }

    let mut wallet = WalletState::load_with_session(
        wallet_path.clone(),
        OperatingMode::SentientTrader,
        "sentient",
    )?;
    wallet.unlock(&password)?;
    let active_address = wallet
        .active_address()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .parse::<Address>()?;
    let chain_id = wallet.networks().active().chain_id;
    let (rpc_url, _) = wallet.rpc_endpoints_for(wallet.networks().active());

    let listener = bind_socket(&socket)?;

    let wallet_arc = Arc::new(tokio::sync::Mutex::new(wallet));
    let broadcaster = Arc::new(broadcaster::WalletBroadcaster::new(wallet_arc));
    let signer = Signer::new(
        store,
        active_address,
        chain_id,
        rpc_url,
        Arc::new(quoter::LiveSwapQuoter),
        broadcaster,
        Arc::new(balance::LiveBalanceSource),
    );

    signer.allow_mainnet_writes(vaughan_core::core::mcp_mainnet_writes_allowed());
    eprintln!(
        "vaughan signerd: active_address={active_address:#x} chain_id={chain_id} socket={} allow_uid={allow_uid:?}",
        socket.display()
    );

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("accept error: {e}");
                continue;
            }
        };

        let signer = signer.clone();
        let allowed = allow_uid.clone();

        tokio::spawn(async move {
            #[cfg(unix)]
            let peer_uid = match stream.peer_cred() {
                Ok(cred) => {
                    let uid = cred.uid();
                    if !is_peer_uid_allowed(uid, &allowed) {
                        tracing::warn!("peer UID {uid} rejected by allowlist {allowed:?}");
                        let resp = serde_json::to_string(&SignerResponse::error(
                            "unauthorized",
                            "peer UID not permitted",
                        ))
                        .unwrap_or_default()
                            + "\n";
                        let _ = stream.write_all(resp.as_bytes()).await;
                        return;
                    }
                    uid
                }
                Err(e) => {
                    tracing::warn!("could not read SO_PEERCRED: {e}");
                    let resp = serde_json::to_string(&SignerResponse::error(
                        "unauthorized",
                        "could not verify peer credentials",
                    ))
                    .unwrap_or_default()
                        + "\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                    return;
                }
            };
            #[cfg(not(unix))]
            let peer_uid: u32 = 0;

            let mut reader = tokio::io::BufReader::new(&mut stream);
            let mut line = String::new();
            use tokio::io::AsyncBufReadExt;
            match reader.read_line(&mut line).await {
                Ok(0) => return,
                Ok(n) if n > 65536 => {
                    line.zeroize();
                    let resp = serde_json::to_string(&SignerResponse::error(
                        "line_too_long",
                        "request line exceeded 64 KiB limit",
                    ))
                    .unwrap_or_default()
                        + "\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    line.zeroize();
                    tracing::error!("socket read error: {e}");
                    return;
                }
            }

            let response_str = signer.handle_line_with_uid(&line, peer_uid).await + "\n";
            line.zeroize();
            let _ = stream.write_all(response_str.as_bytes()).await;
        });
    }
}

/// Bind the daemon's Unix socket: replace a stale socket file, then restrict it to mode
/// `0660` and, when a `vaughan` group exists, hand it to that group so its members (and
/// only they, plus the allowlisted peer UIDs) can connect.
fn bind_socket(socket: &Path) -> anyhow::Result<tokio::net::UnixListener> {
    #[cfg(unix)]
    {
        if let Some(parent) = socket.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if socket.exists() {
            let _ = std::fs::remove_file(socket);
        }
    }

    let listener = tokio::net::UnixListener::bind(socket)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660)) {
            tracing::warn!("failed to set socket mode 0660: {e}");
        }
        if let Some(gid) = find_vaughan_gid() {
            if let Err(e) = std::os::unix::fs::chown(socket, None, Some(gid)) {
                tracing::info!(
                    "could not chgrp socket {} to vaughan (gid {gid}): {e}; continuing with current group",
                    socket.display()
                );
            }
        }
    }
    Ok(listener)
}
