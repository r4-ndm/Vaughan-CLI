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
use vaughan_core::core::{DexVenue, WalletState};
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
