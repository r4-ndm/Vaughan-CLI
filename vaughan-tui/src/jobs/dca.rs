//! Body for the [`crate::jobs::UiJob::DcaSlice`] job, lifted out of `app.rs`'s
//! `spawn_job`.
//!
//! Single purpose: fire one due DCA slice. Under a sentient profile this
//! strictly routes via `SignerClient` over the Unix socket submitting a typed
//! [`vaughan_core::core::SignerIntent::DcaSlice`]; local in-process signing
//! and local `SentientTrader` construction are forbidden there — see
//! `docs/audit-playbook.md` invariant 8 (the agent key cannot withdraw outside
//! a quoted intent). Non-sentient profiles go through `SentientTrader` with a
//! locally-held signer instead, same as the standalone `vaughan-hl` bot.

use vaughan_core::core::{SignerClient, SignerIntent, WalletState};
use vaughan_core::error::WalletError;

use super::UiJobResult;

/// Execute a single due DCA slice job.
pub fn execute_dca_slice_job(
    wallet: &std::sync::Mutex<WalletState>,
    handle: &tokio::runtime::Handle,
    plan_id: String,
    profile_dir: std::path::PathBuf,
    breaker: vaughan_agent::CircuitBreaker,
) -> UiJobResult {
    let is_sentient = {
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        vaughan_core::core::is_sentient_profile(w.profile_name())
    };
    if is_sentient {
        if breaker.is_tripped() {
            let reason = breaker
                .trip_reason()
                .unwrap_or_else(|| "circuit breaker tripped".into());
            return UiJobResult::DcaSlice(Err(WalletError::Other(format!(
                "circuit_breaker_tripped: {reason}"
            ))));
        }
        let (chain_id, rpc_url) = {
            let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
            (w.networks().active().chain_id, w.active_rpc_url())
        };
        let plans_file = match vaughan_agent::dca::store::load_plans(&profile_dir) {
            Ok(f) => f,
            Err(e) => return UiJobResult::DcaSlice(Err(WalletError::Other(e.to_string()))),
        };
        let mut plan = match plans_file.plans.into_iter().find(|p| p.id == plan_id) {
            Some(p) => p,
            None => {
                return UiJobResult::DcaSlice(Err(WalletError::Other(format!(
                    "dca plan `{plan_id}` not found"
                ))))
            }
        };
        let now = vaughan_agent::now_unix();
        if !vaughan_agent::dca::is_due(&plan, now) {
            return UiJobResult::DcaSlice(Err(WalletError::Other(format!(
                "plan `{plan_id}` is not due yet"
            ))));
        }
        if plan.chain_id != 0 && plan.chain_id != chain_id {
            return UiJobResult::DcaSlice(Err(WalletError::Other(format!(
                "plan `{plan_id}` bound to chain {} — active chain is {chain_id}",
                plan.chain_id
            ))));
        }
        let amount_in = match plan.slice_amount_u256() {
            Ok(a) => a,
            Err(e) => return UiJobResult::DcaSlice(Err(WalletError::Other(e))),
        };
        let (pair, corridor_min_out) = match handle.block_on(
            vaughan_agent::resolve_plan_pair_and_corridor(&rpc_url, chain_id, &plan),
        ) {
            Ok(res) => res,
            Err(e) => {
                let msg = e.to_string();
                let _ = vaughan_agent::dca::store::update_plan(&profile_dir, &plan.id, |p| {
                    p.consecutive_failures = p.consecutive_failures.saturating_add(1);
                    if p.consecutive_failures >= vaughan_agent::dca::MAX_CONSECUTIVE_FAILURES {
                        p.status = vaughan_agent::dca::DcaStatus::Paused;
                    }
                    vaughan_agent::dca::advance_schedule(p, now);
                    p.slice_log.push(vaughan_agent::dca::SliceLog {
                        at: now,
                        ok: false,
                        tx_hash: String::new(),
                        dry_run: plan.dry_run,
                        error: msg.clone(),
                        amount_in_wei: amount_in.to_string(),
                    });
                    if p.slice_log.len() > 32 {
                        let drop_n = p.slice_log.len() - 32;
                        p.slice_log.drain(0..drop_n);
                    }
                    Ok(())
                });
                return UiJobResult::DcaSlice(Err(WalletError::Other(msg)));
            }
        };

        if plan.pair.as_deref() != Some(&format!("{pair:#x}")) {
            let _ = vaughan_agent::dca::store::update_plan(&profile_dir, &plan.id, |p| {
                p.pair = Some(format!("{pair:#x}"));
                Ok(())
            });
            plan.pair = Some(format!("{pair:#x}"));
        }

        let intent = SignerIntent::DcaSlice {
            plan_id: plan.id.clone(),
            slice_index: plan.slices_done as u64,
            chain_id,
            pair,
            amount_in,
            corridor_min_out,
        };
        let client = SignerClient::default();
        let exec_res = handle.block_on(client.execute(&intent));
        let result = match exec_res {
            Ok(tx_hash) => {
                let _ = vaughan_agent::dca::store::update_plan(&profile_dir, &plan.id, |p| {
                    p.consecutive_failures = 0;
                    p.slices_done = p.slices_done.saturating_add(1);
                    if let Ok(spent) = p.spent_u256() {
                        p.spent_wei = spent.saturating_add(amount_in).to_string();
                    }
                    vaughan_agent::dca::advance_schedule(p, now);
                    if p.slices_done >= p.max_slices {
                        p.status = vaughan_agent::dca::DcaStatus::Done;
                    }
                    p.slice_log.push(vaughan_agent::dca::SliceLog {
                        at: now,
                        ok: true,
                        tx_hash: tx_hash.clone(),
                        dry_run: plan.dry_run,
                        error: String::new(),
                        amount_in_wei: amount_in.to_string(),
                    });
                    if p.slice_log.len() > 32 {
                        let drop_n = p.slice_log.len() - 32;
                        p.slice_log.drain(0..drop_n);
                    }
                    Ok(())
                });
                Ok(vaughan_agent::DcaFireResult {
                    plan_id: plan.id.clone(),
                    ok: true,
                    dry_run: plan.dry_run,
                    tx_hash,
                    error: String::new(),
                })
            }
            Err(refusal) => {
                // A gas refusal moved no money: it must not count as a failure or burn
                // this buy window (see crate::dca_defer).
                let class = crate::dca_defer::classify(&refusal);
                let _ = vaughan_agent::dca::store::update_plan(&profile_dir, &plan.id, |p| {
                    vaughan_agent::dca::apply_refusal(p, now, class, &refusal, &amount_in);
                    Ok(())
                });
                Err(WalletError::Other(refusal))
            }
        };
        UiJobResult::DcaSlice(result)
    } else {
        let (signer, rpcs, chain_id, profile_name) = {
            let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
            let signer = w.active_signer().map_err(|e| e.to_string());
            let mut rpcs = vec![w.active_rpc_url()];
            rpcs.extend(w.networks().active().fallback_rpc_urls.iter().cloned());
            let chain_id = w.networks().active().chain_id;
            let profile_name = w.profile_name().to_string();
            (signer, rpcs, chain_id, profile_name)
        };
        let result = (|| {
            let signer = signer.map_err(WalletError::Other)?;
            let trader = vaughan_agent::SentientTrader::with_breaker(
                &profile_name,
                signer,
                rpcs,
                chain_id,
                breaker,
            )
            .map_err(|e| WalletError::Other(e.to_string()))?
            .with_profile_dir(&profile_dir);
            let now = vaughan_agent::now_unix();
            handle
                .block_on(vaughan_agent::fire_plan(
                    &profile_dir,
                    &plan_id,
                    &trader,
                    chain_id,
                    Some(profile_dir.as_path()),
                    None,
                    now,
                ))
                .map_err(|e| WalletError::Other(e.to_string()))
        })();
        UiJobResult::DcaSlice(result)
    }
}
