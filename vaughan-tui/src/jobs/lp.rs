//! Bodies for the LP background jobs, lifted out of `app.rs`'s `spawn_job`.
//!
//! Single purpose: run one LP [`crate::jobs::UiJob`] to its
//! [`crate::jobs::UiJobResult`]. These run on the plain `std::thread` that
//! `spawn_job` spawns, so each takes the tokio [`Handle`] explicitly and uses
//! `block_on` — there is no ambient runtime on that thread.
//!
//! Every function here takes an already-resolved RPC URL list (see [`rpc_urls`])
//! rather than the wallet, which is what keeps the wallet mutex off the RPC
//! await: `spawn_job` locks the wallet just long enough to read the endpoints and
//! drops it before calling in. Holding it across these awaits freezes the render
//! loop, and with it the spinner.
//!
//! Must NOT contain: view state, job construction (that is `views::lp::jobs`),
//! or channel plumbing.

use tokio::runtime::Handle;
use vaughan_core::core::{DexVenue, V3LpDeployParams, V3LpDeployWait, WalletState};
use vaughan_core::error::WalletError;

use super::UiJobResult;

/// The endpoints an LP job should try, in order.
///
/// Prefers the active network's full fallback list so an LP call gets the same
/// multi-endpoint treatment as the rest of the wallet; falls back to the single
/// URL the job was built with when the wallet cannot answer.
pub(crate) fn rpc_urls(wallet: &WalletState, job_primary: &str) -> Vec<String> {
    wallet
        .active_rpc_url_list()
        .unwrap_or_else(|_| vaughan_core::core::merge_rpc_urls(job_primary, &[]))
}

/// Parse a view-supplied token address, naming the field in the error.
///
/// Trims, because these arrive from text input the user may have padded.
fn parse_addr(s: &str, label: &str) -> Result<alloy::primitives::Address, WalletError> {
    use std::str::FromStr;
    alloy::primitives::Address::from_str(s.trim())
        .map_err(|_| WalletError::InvalidTransaction(format!("invalid {label}")))
}

/// Parse the owner address of a listing job.
///
/// Deliberately does NOT trim, unlike [`parse_addr`]: the owner comes from the
/// account the wallet selected, never from typing, so whitespace means something
/// upstream is wrong and should surface rather than be silently accepted.
fn parse_owner(s: &str) -> Result<alloy::primitives::Address, WalletError> {
    use std::str::FromStr;
    alloy::primitives::Address::from_str(s)
        .map_err(|_| WalletError::InvalidTransaction("invalid owner address".into()))
}

/// List V3 LP positions for `owner`.
pub(crate) fn list_v3_positions(
    handle: &Handle,
    rpc_urls: &[String],
    venue: DexVenue,
    chain_id: u64,
    owner: String,
    list_gen: u64,
) -> UiJobResult {
    use vaughan_core::core::{list_v3_lp_position_views, with_lp_rpc_urls};
    let owner_for_result = owner.clone();
    UiJobResult::LpPositions {
        list_gen,
        owner: owner_for_result,
        result: match parse_owner(&owner) {
            Err(e) => Err(e),
            Ok(addr) => handle.block_on(with_lp_rpc_urls(rpc_urls, |url| async move {
                list_v3_lp_position_views(&url, venue, chain_id, addr, None, None).await
            })),
        },
    }
}

/// List V2 LP pair balances for `owner`.
///
/// Listing always walks every V2 factory on the chain so ETHW LPs aren't hidden
/// behind the current `[ ]` DEX focus — which is why this takes no venue.
pub(crate) fn list_v2_positions(
    handle: &Handle,
    rpc_urls: &[String],
    chain_id: u64,
    owner: String,
    list_gen: u64,
) -> UiJobResult {
    use vaughan_core::core::{list_all_v2_lp_positions, with_lp_rpc_urls};
    let owner_for_result = owner.clone();
    UiJobResult::LpV2Positions {
        list_gen,
        owner: owner_for_result,
        result: match parse_owner(&owner) {
            Err(e) => Err(e),
            Ok(addr) => handle.block_on(with_lp_rpc_urls(rpc_urls, |url| async move {
                list_all_v2_lp_positions(&url, chain_id, addr).await
            })),
        },
    }
}

/// Build the next tx in the V3 create → initialize → approve → mint pipeline.
///
/// `deploy_wait` first blocks on the step the user just broadcast, so the next
/// tx is built against post-inclusion chain state; the timeout therefore covers
/// that wait as well as the build.
pub(crate) fn v3_pool_deploy_step(
    handle: &Handle,
    rpc_urls: &[String],
    params: V3LpDeployParams,
    deploy_wait: V3LpDeployWait,
    after_step_label: Option<String>,
) -> UiJobResult {
    use vaughan_core::core::{
        v3_lp_prepare_deploy_step, v3_lp_run_deploy_wait, with_lp_rpc_urls, V3LpDeployContext,
    };
    UiJobResult::LpV3PoolDeployStep(handle.block_on(async {
        use tokio::time::{timeout, Duration};
        const DEPLOY_PREP_TIMEOUT: Duration = Duration::from_secs(30);
        const DEPLOY_ONCHAIN_WAIT: Duration = Duration::from_secs(60);
        let job_timeout = match deploy_wait {
            V3LpDeployWait::None => DEPLOY_PREP_TIMEOUT,
            _ => DEPLOY_ONCHAIN_WAIT + DEPLOY_PREP_TIMEOUT,
        };
        with_lp_rpc_urls(rpc_urls, |url| {
            let mut p = params.clone();
            p.rpc_url = url;
            let ctx = after_step_label.clone().map(|label| V3LpDeployContext {
                last_step_label: Some(label),
            });
            async move {
                timeout(job_timeout, async {
                    v3_lp_run_deploy_wait(deploy_wait, &p, ctx.as_ref()).await?;
                    v3_lp_prepare_deploy_step(&p).await
                })
                .await
                .map_err(|_| {
                    WalletError::NetworkError(format!(
                        "LP deploy step timed out ({}s)",
                        job_timeout.as_secs()
                    ))
                })?
            }
        })
        .await
    }))
}

/// Read whether the NPM is already enabled (approved) for both deposit legs.
pub(crate) fn enable_check(
    handle: &Handle,
    rpc_urls: &[String],
    params: V3LpDeployParams,
) -> UiJobResult {
    use vaughan_core::core::{v3_lp_token_enable_status, with_lp_rpc_urls};
    UiJobResult::LpEnableCheck(handle.block_on(async {
        use tokio::time::{timeout, Duration};
        const ENABLE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
        with_lp_rpc_urls(rpc_urls, |url| {
            let mut p = params.clone();
            p.rpc_url = url;
            async move {
                timeout(ENABLE_CHECK_TIMEOUT, v3_lp_token_enable_status(&p))
                    .await
                    .map_err(|_| WalletError::NetworkError("enable check timed out (30s)".into()))?
            }
        })
        .await
    }))
}

/// Build the next Enable (approve) tx, if the pool is far enough along to need one.
pub(crate) fn enable_prepare(
    handle: &Handle,
    rpc_urls: &[String],
    params: V3LpDeployParams,
    symbol: String,
) -> UiJobResult {
    use vaughan_core::core::{v3_lp_build_next_enable_tx, with_lp_rpc_urls};
    UiJobResult::LpEnablePrepare(handle.block_on(async {
        use tokio::time::{timeout, Duration};
        const ENABLE_PREP_TIMEOUT: Duration = Duration::from_secs(30);
        let out = with_lp_rpc_urls(rpc_urls, |url| {
            let mut p = params.clone();
            p.rpc_url = url;
            async move {
                timeout(ENABLE_PREP_TIMEOUT, v3_lp_build_next_enable_tx(&p))
                    .await
                    .map_err(|_| {
                        WalletError::NetworkError("enable prepare timed out (30s)".into())
                    })?
            }
        })
        .await?;
        match out {
            Some((tx, label)) => Ok((tx, label, symbol)),
            None => Err(WalletError::InvalidTransaction(
                "pool not ready for Enable yet — finish pool setup first".into(),
            )),
        }
    }))
}

/// Wait for a broadcast Enable to be included, so the form can re-check allowances.
pub(crate) fn enable_wait(
    handle: &Handle,
    rpc_urls: &[String],
    params: V3LpDeployParams,
    after_step_label: String,
) -> UiJobResult {
    use vaughan_core::core::{v3_lp_run_deploy_wait, with_lp_rpc_urls, V3LpDeployContext};
    UiJobResult::LpEnableWait(handle.block_on(async {
        use tokio::time::{timeout, Duration};
        const ENABLE_WAIT: Duration = Duration::from_secs(75);
        let ctx = V3LpDeployContext {
            last_step_label: Some(after_step_label),
        };
        with_lp_rpc_urls(rpc_urls, |url| {
            let mut p = params.clone();
            p.rpc_url = url;
            let ctx = ctx.clone();
            async move {
                timeout(
                    ENABLE_WAIT,
                    v3_lp_run_deploy_wait(V3LpDeployWait::AfterApprove, &p, Some(&ctx)),
                )
                .await
                .map_err(|_| WalletError::NetworkError("Enable wait timed out (75s)".into()))?
            }
        })
        .await
    }))
}

/// Look up the V3 pool behind a prospective add-liquidity position.
#[allow(clippy::too_many_arguments)]
pub(crate) fn v3_pool_quote(
    handle: &Handle,
    rpc_urls: &[String],
    venue: DexVenue,
    chain_id: u64,
    token0: String,
    token1: String,
    fee: u32,
    dec0: u8,
    dec1: u8,
) -> UiJobResult {
    use vaughan_core::core::{fetch_v3_lp_pool_quote, with_lp_rpc_urls};
    UiJobResult::LpV3PoolQuote(handle.block_on(async {
        use tokio::time::{timeout, Duration};
        const POOL_QUOTE_TIMEOUT: Duration = Duration::from_secs(45);
        let t0 = parse_addr(&token0, "token0")?;
        let t1 = parse_addr(&token1, "token1")?;
        if t0 >= t1 {
            return Err(WalletError::InvalidTransaction(
                "internal: token0 must be sorted".into(),
            ));
        }
        with_lp_rpc_urls(rpc_urls, |url| async move {
            timeout(
                POOL_QUOTE_TIMEOUT,
                fetch_v3_lp_pool_quote(&url, venue, chain_id, t0, t1, dec0, dec1, fee),
            )
            .await
            .map_err(|_| WalletError::NetworkError("pool lookup timed out (45s)".into()))?
        })
        .await
    }))
}
