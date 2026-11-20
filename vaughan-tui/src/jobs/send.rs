//! Bodies for the native/token/contract send and fee-estimate
//! [`crate::jobs::UiJob`]s, lifted out of `app.rs`'s `spawn_job`.
//!
//! This is the actual signing/broadcast dispatch — see
//! `docs/audit-playbook.md` invariant 4 (custody stays with the crate that
//! holds the wallet) before changing anything here.
//!
//! Lock discipline: the native/token/EVM send paths lock the wallet only long
//! enough to build the unsigned tx and read a sign context, then drop it before
//! the broadcast await. [`send_stealth`] is the exception — it holds the guard
//! across its whole `block_on(w.send_stealth(..))`, i.e. across a sign **and**
//! broadcast, which blocks every other wallet access for that window. Carried
//! over unchanged from `spawn_job`; tracked in `TASKS.md`.
//!
//! Sentient-profile sends route through [`execute_sentient_transfer_job`] /
//! [`execute_sentient_evm_job`] instead of `detached_sign_context` — those go
//! through the Rust circuit breakers in `vaughan_core::core::SignerClient`,
//! not a locally-held key.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use alloy::primitives::{Address, U256};
use tokio::runtime::Handle;
use vaughan_agent::paths::profile_dir;
use vaughan_core::chains::{ChainAdapter, EvmTransaction, Fee};
use vaughan_core::core::{
    entitlement_chain_id, vault_has_assist_burn_with_retry, BroadcastEntry, BroadcastReceipt,
    SignerClient, SignerIntent, WalletState,
};
use vaughan_core::error::WalletError;
use vaughan_core::security::stealth::StealthAnnouncement;

use super::UiJobResult;

pub(crate) fn estimate_fee(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    to: String,
    value_wei: String,
) -> UiJobResult {
    let prepared = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        (|| {
            let tx = w.build_native_transfer(&to, &value_wei)?;
            let snap = w.network_rpc_snapshot()?;
            Ok::<_, WalletError>((tx, snap))
        })()
    };
    UiJobResult::Fee(match prepared {
        Ok((tx, snap)) => handle.block_on(async move {
            let adapter = snap.adapter().await?;
            adapter
                .estimate_fee(&vaughan_core::chains::ChainTransaction::Evm(tx))
                .await
        }),
        Err(e) => Err(e),
    })
}

pub(crate) fn estimate_token_fee(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    token: String,
    to: String,
    amount: String,
) -> UiJobResult {
    let prepared = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        (|| {
            let tx = w.build_erc20_transfer(&token, &to, &amount)?;
            let snap = w.network_rpc_snapshot()?;
            Ok::<_, WalletError>((tx, snap))
        })()
    };
    UiJobResult::Fee(match prepared {
        Ok((tx, snap)) => handle.block_on(async move {
            let adapter = snap.adapter().await?;
            adapter
                .estimate_fee(&vaughan_core::chains::ChainTransaction::Evm(tx))
                .await
        }),
        Err(e) => Err(e),
    })
}

pub(crate) fn send_with_fee(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    to: String,
    value_wei: String,
    fee: Fee,
) -> UiJobResult {
    let is_sentient = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        vaughan_core::core::is_sentient_profile(w.profile_name())
    };
    if is_sentient {
        execute_sentient_transfer_job(wallet, handle, to, value_wei, None, "Send")
    } else {
        let prepared = {
            let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
            (|| {
                let tx = w.build_native_transfer_with_fee(&to, &value_wei, &fee)?;
                let ctx = w.detached_sign_context()?;
                Ok::<_, WalletError>((tx, ctx))
            })()
        };
        UiJobResult::Send(match prepared {
            Ok((tx, ctx)) => handle.block_on(ctx.broadcast(tx, "Send")),
            Err(e) => Err(e),
        })
    }
}

pub(crate) fn send(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    to: String,
    value_wei: String,
) -> UiJobResult {
    let is_sentient = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        vaughan_core::core::is_sentient_profile(w.profile_name())
    };
    if is_sentient {
        execute_sentient_transfer_job(wallet, handle, to, value_wei, None, "Send")
    } else {
        let prepared = {
            let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
            (|| {
                let tx = w.build_native_transfer(&to, &value_wei)?;
                let ctx = w.detached_sign_context()?;
                Ok::<_, WalletError>((tx, ctx))
            })()
        };
        UiJobResult::Send(match prepared {
            Ok((tx, ctx)) => handle.block_on(ctx.broadcast(tx, "Send")),
            Err(e) => Err(e),
        })
    }
}

pub(crate) fn send_token(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    token: String,
    to: String,
    amount: String,
) -> UiJobResult {
    let is_sentient = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        vaughan_core::core::is_sentient_profile(w.profile_name())
    };
    if is_sentient {
        execute_sentient_transfer_job(wallet, handle, to, amount, Some(token), "Token")
    } else {
        let prepared = {
            let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
            (|| {
                let tx = w.build_erc20_transfer(&token, &to, &amount)?;
                let ctx = w.detached_sign_context()?;
                Ok::<_, WalletError>((tx, ctx))
            })()
        };
        UiJobResult::Send(match prepared {
            Ok((tx, ctx)) => handle.block_on(ctx.broadcast(tx, "Token")),
            Err(e) => Err(e),
        })
    }
}

pub(crate) fn send_token_with_fee(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    token: String,
    to: String,
    amount: String,
    fee: Fee,
) -> UiJobResult {
    let is_sentient = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        vaughan_core::core::is_sentient_profile(w.profile_name())
    };
    if is_sentient {
        execute_sentient_transfer_job(wallet, handle, to, amount, Some(token), "Token")
    } else {
        let prepared = {
            let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
            (|| {
                let tx = w.build_erc20_transfer_with_fee(&token, &to, &amount, &fee)?;
                let ctx = w.detached_sign_context()?;
                Ok::<_, WalletError>((tx, ctx))
            })()
        };
        UiJobResult::Send(match prepared {
            Ok((tx, ctx)) => handle.block_on(ctx.broadcast(tx, "Token")),
            Err(e) => Err(e),
        })
    }
}

pub(crate) fn send_stealth(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    announcement: StealthAnnouncement,
    value_wei: String,
) -> UiJobResult {
    let is_sentient = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        vaughan_core::core::is_sentient_profile(w.profile_name())
    };
    if is_sentient {
        UiJobResult::SendStealth(Err(WalletError::Other(
            "intent_untyped: stealth send requires human approval via an adviser profile (not available on sentient)".into(),
        )))
    } else {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        UiJobResult::SendStealth(handle.block_on(w.send_stealth(&announcement, &value_wei)))
    }
}

pub(crate) fn send_evm(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    tx: EvmTransaction,
) -> UiJobResult {
    let is_sentient = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        vaughan_core::core::is_sentient_profile(w.profile_name())
    };
    if is_sentient {
        execute_sentient_evm_job(handle, tx, "Contract")
    } else {
        let prepared = {
            let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
            w.detached_sign_context().map(|ctx| (tx, ctx))
        };
        UiJobResult::Send(match prepared {
            Ok((tx, ctx)) => handle.block_on(ctx.broadcast(tx, "Contract")),
            Err(e) => Err(e),
        })
    }
}

pub(crate) fn send_evm_with_fee(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    tx: EvmTransaction,
    fee: Fee,
) -> UiJobResult {
    let is_sentient = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        vaughan_core::core::is_sentient_profile(w.profile_name())
    };
    if is_sentient {
        execute_sentient_evm_job(handle, tx, "Contract")
    } else {
        let prepared = {
            let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
            (|| {
                let tx = w.apply_fee_to_evm(tx, &fee)?;
                let ctx = w.detached_sign_context()?;
                Ok::<_, WalletError>((tx, ctx))
            })()
        };
        UiJobResult::Send(match prepared {
            Ok((tx, ctx)) => handle.block_on(ctx.broadcast(tx, "Contract")),
            Err(e) => Err(e),
        })
    }
}

pub(crate) fn assist_burn_verify(wallet: &Arc<Mutex<WalletState>>, handle: &Handle) -> UiJobResult {
    let (dir, addrs) = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        let dir = profile_dir(w.path());
        let addrs = w.account_addresses().unwrap_or_default();
        (dir, addrs)
    };
    UiJobResult::AssistBurnVerify(match entitlement_chain_id() {
        Some(chain_id) => handle.block_on(vault_has_assist_burn_with_retry(
            Some(&dir),
            chain_id,
            &addrs,
        )),
        None => Ok(false),
    })
}

pub(crate) fn estimate_evm_fee(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    tx: EvmTransaction,
) -> UiJobResult {
    let prepared = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        w.network_rpc_snapshot().map(|snap| (tx, snap))
    };
    UiJobResult::Fee(match prepared {
        Ok((tx, snap)) => handle.block_on(async move {
            let adapter = snap.adapter().await?;
            adapter
                .estimate_fee(&vaughan_core::chains::ChainTransaction::Evm(tx))
                .await
        }),
        Err(e) => Err(e),
    })
}

pub(crate) fn deploy_token(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    name: String,
    symbol: String,
    supply: String,
) -> UiJobResult {
    UiJobResult::DeployToken(
        handle.block_on(WalletState::deploy_fixed_supply_token_background(
            wallet, &name, &symbol, &supply,
        )),
    )
}

/// Sentient-profile native/token transfer: routed through
/// [`SignerClient`]/`SignerIntent` (Rust circuit breakers), never
/// `detached_sign_context` — see `docs/audit-playbook.md` invariant 8 (the
/// agent key cannot withdraw outside a quoted intent).
fn execute_sentient_transfer_job(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    to: String,
    value_wei: String,
    token: Option<String>,
    label: &'static str,
) -> UiJobResult {
    let (chain_id, from) = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        (
            w.networks().active().chain_id,
            w.active_address()
                .map(|a| a.to_string())
                .unwrap_or_default(),
        )
    };
    let res = handle.block_on(async {
        let recipient =
            Address::from_str(&to).map_err(|e| WalletError::InvalidTransaction(e.to_string()))?;
        let amount = U256::from_str(&value_wei)
            .map_err(|_| WalletError::InvalidAmount("bad value".into()))?;
        let token_addr = token
            .as_deref()
            .map(Address::from_str)
            .transpose()
            .map_err(|e| WalletError::InvalidTransaction(e.to_string()))?;
        let intent = SignerIntent::Transfer {
            chain_id,
            to: recipient,
            token: token_addr,
            amount,
        };
        let client = SignerClient::default();
        let hash = client.execute(&intent).await.map_err(WalletError::Other)?;
        let tx = EvmTransaction {
            from,
            to: if let Some(ref t) = token {
                t.clone()
            } else {
                to
            },
            value: if token.is_some() {
                "0".into()
            } else {
                value_wei
            },
            data: None,
            chain_id,
            nonce: None,
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };
        Ok(BroadcastReceipt {
            entry: BroadcastEntry::from_prepared(&tx, hash.clone(), label),
            hash,
        })
    });
    UiJobResult::Send(res)
}

/// Sentient-profile arbitrary EVM call, same custody boundary as
/// [`execute_sentient_transfer_job`].
fn execute_sentient_evm_job(
    handle: &Handle,
    tx: EvmTransaction,
    label: &'static str,
) -> UiJobResult {
    let res = handle.block_on(async {
        let intent = crate::sentient_mcp::evm_tx_to_sentient_intent(&tx)?;
        let client = SignerClient::default();
        let hash = client.execute(&intent).await.map_err(WalletError::Other)?;
        Ok(BroadcastReceipt {
            entry: BroadcastEntry::from_prepared(&tx, hash.clone(), label),
            hash,
        })
    });
    UiJobResult::Send(res)
}
