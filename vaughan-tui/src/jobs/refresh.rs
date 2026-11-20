//! Bodies for the read-only refresh/status [`crate::jobs::UiJob`]s, lifted out
//! of `app.rs`'s `spawn_job`.
//!
//! Single purpose: run one refresh/poll job to its [`crate::jobs::UiJobResult`].
//! These run on the plain `std::thread` that `spawn_job` spawns, so each takes
//! the tokio [`Handle`] explicitly and uses `block_on` — there is no ambient
//! runtime on that thread.
//!
//! Lock discipline: most functions here lock the wallet only long enough to
//! read a snapshot/endpoint and drop it before the RPC await, matching the rule
//! `spawn_job` states ("never hold the wallet mutex across RPC / signing
//! awaits"). Three do **not** — [`refresh_activity`], [`refresh_allowances`]
//! and [`replace_broadcast`] hold the guard across `block_on`, carried over
//! unchanged from `spawn_job`. Any other wallet access blocks for the duration
//! of those calls; tracked in `TASKS.md` rather than changed here, because
//! re-scoping them is a behaviour change, not a move.

use std::sync::{Arc, Mutex};

use secrecy::SecretString;
use tokio::runtime::Handle;
use vaughan_core::core::{BroadcastEntry, OperatingMode, ReplaceKind, WalletState};

use super::{UiJobResult, UnlockCompletion};

pub(crate) fn unlock(
    wallet: &Arc<Mutex<WalletState>>,
    password: SecretString,
    mode: OperatingMode,
) -> UiJobResult {
    // Clone the vault under a brief lock, then run the Argon2id KDF unlocked —
    // holding the mutex across it would freeze the render loop (and the
    // spinner with it).
    let payload = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        w.unlock_payload()
    };
    UiJobResult::Unlock(match payload {
        Ok(p) => p
            .decrypt(&password)
            .map(|accounts| UnlockCompletion { accounts, mode }),
        Err(e) => Err(e),
    })
}

pub(crate) fn refresh_chrome(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    owner: String,
    gen: u64,
) -> UiJobResult {
    let owner_for_result = owner.clone();
    let snap = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        w.chrome_rpc_snapshot_for(&owner)
    };
    UiJobResult::Chrome {
        owner: owner_for_result,
        gen,
        result: match snap {
            Ok(s) => handle.block_on(s.fetch_chrome()),
            Err(e) => Err(e),
        },
    }
}

pub(crate) fn refresh_balance(wallet: &Arc<Mutex<WalletState>>, handle: &Handle) -> UiJobResult {
    let snap = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        w.chrome_rpc_snapshot()
    };
    UiJobResult::Balance(match snap {
        Ok(s) => handle.block_on(s.balance()),
        Err(e) => Err(e),
    })
}

pub(crate) fn refresh_assets(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    owner: String,
    gen: u64,
) -> UiJobResult {
    let owner_for_result = owner.clone();
    let (snap, extras) = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        (
            w.chrome_rpc_snapshot_for(&owner),
            w.custom_token_addresses_for_active_chain(),
        )
    };
    UiJobResult::Assets {
        owner: owner_for_result,
        gen,
        result: match snap {
            Ok(s) => handle.block_on(s.fetch_assets(&extras)),
            Err(e) => Err(e),
        },
    }
}

pub(crate) fn refresh_activity(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    limit: u32,
) -> UiJobResult {
    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
    UiJobResult::Activity(handle.block_on(w.activity(limit)))
}

pub(crate) fn refresh_allowances(wallet: &Arc<Mutex<WalletState>>, handle: &Handle) -> UiJobResult {
    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
    UiJobResult::Allowances(handle.block_on(w.list_allowances()))
}

pub(crate) fn refresh_hex_stakes(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    owner: String,
    gen: u64,
) -> UiJobResult {
    let rpc = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        w.active_rpc_url()
    };
    let (stakes, globals) = handle.block_on(crate::views::hex::load_hex_stakes(&rpc, &owner));
    UiJobResult::HexStakes {
        gen,
        owner,
        stakes,
        globals,
    }
}

pub(crate) fn poll_tx_status(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    tx_hash: String,
) -> UiJobResult {
    // Brief lock for RPC endpoints only — holding WalletState across
    // eth_getTransactionReceipt freezes the Done screen on "working…".
    let snap = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        w.network_rpc_snapshot()
    };
    UiJobResult::TxStatus(match snap {
        Ok(s) => handle.block_on(s.get_tx_status(&tx_hash)),
        Err(e) => Err(e),
    })
}

pub(crate) fn refresh_broadcast_statuses(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    hashes: Vec<String>,
) -> UiJobResult {
    let snap = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        w.network_rpc_snapshot()
    };
    UiJobResult::BroadcastStatuses(match snap {
        Ok(s) => {
            let mut out = Vec::with_capacity(hashes.len());
            let mut err = None;
            for h in hashes {
                match handle.block_on(s.get_tx_status(&h)) {
                    Ok(st) => out.push((h, st)),
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }
            match err {
                Some(e) => Err(e),
                None => Ok(out),
            }
        }
        Err(e) => Err(e),
    })
}

pub(crate) fn replace_broadcast(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    entry: BroadcastEntry,
    kind: ReplaceKind,
) -> UiJobResult {
    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
    UiJobResult::Send(handle.block_on(w.replace_broadcast(&entry, kind)))
}
