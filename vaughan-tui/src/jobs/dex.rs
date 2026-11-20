//! Bodies for the DEX/aggregator/bridge quote and allowance
//! [`crate::jobs::UiJob`]s, lifted out of `app.rs`'s `spawn_job`.
//!
//! Read-only: these jobs quote or check allowance, they never sign or
//! broadcast (that is `jobs::send`'s `UiJob::SendEvm*` once the view has a
//! quote it wants to act on).
//!
//! Lock discipline: all of these are lock-free except
//! [`dex_swap_estimate_after_approve`], which holds the wallet guard across its
//! `estimate_transaction_fee` await — carried over unchanged from `spawn_job`
//! and tracked in `TASKS.md`.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use alloy::primitives::{Address, U256};
use tokio::runtime::Handle;
use vaughan_core::chains::Fee;
use vaughan_core::core::{
    discover_v3_swap_route, erc20_allowance_covers, quote_aggregator, quote_live_aggregators,
    quote_v2_exact_in, quote_v3_path_exact_in, resolve_v3_swap_path, wait_erc20_allowance,
    AggQuoteOutcome, AggQuoteRequest, AggVenue, BridgeAsset, BridgeQuoteRequest, DexQuote,
    LibertySwapClient, StateManager, WalletState,
};
use vaughan_core::error::WalletError;

use super::UiJobResult;

fn parse_agg_quote_request(
    token_in: &str,
    token_out: &str,
    amount: &str,
    slippage: f64,
    native_in: bool,
    native_out: bool,
    account: Option<&str>,
) -> Result<AggQuoteRequest, WalletError> {
    let token_in = Address::from_str(token_in)
        .map_err(|_| WalletError::InvalidTransaction("agg token_in".into()))?;
    let token_out = Address::from_str(token_out)
        .map_err(|_| WalletError::InvalidTransaction("agg token_out".into()))?;
    let amount_in =
        U256::from_str(amount).map_err(|_| WalletError::InvalidAmount("agg amount".into()))?;
    let account = account
        .map(Address::from_str)
        .transpose()
        .map_err(|_| WalletError::InvalidTransaction("agg account".into()))?;
    Ok(AggQuoteRequest {
        token_in,
        token_out,
        token_in_is_native: native_in,
        token_out_is_native: native_out,
        amount_in,
        slippage_percent: slippage,
        account,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn dex_swap_estimate_after_approve(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    rpc_url: String,
    token_in: String,
    owner: String,
    router: String,
    amount_in: String,
    tx: vaughan_core::chains::EvmTransaction,
) -> UiJobResult {
    let parsed = (|| -> Result<Fee, WalletError> {
        let token = Address::from_str(token_in.trim())
            .map_err(|_| WalletError::InvalidTransaction("dex token in".into()))?;
        let owner = Address::from_str(owner.trim())
            .map_err(|_| WalletError::InvalidTransaction("dex owner".into()))?;
        let router = Address::from_str(router.trim())
            .map_err(|_| WalletError::InvalidTransaction("dex router".into()))?;
        let need = U256::from_str(&amount_in)
            .map_err(|_| WalletError::InvalidAmount("dex amount".into()))?;
        handle.block_on(wait_erc20_allowance(&rpc_url, token, owner, router, need))?;
        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        handle.block_on(w.estimate_transaction_fee(tx))
    })();
    UiJobResult::Fee(parsed)
}

pub(crate) fn dex_allowance_check(
    handle: &Handle,
    rpc_url: String,
    token_in: String,
    owner: String,
    router: String,
    amount_in: String,
) -> UiJobResult {
    let parsed = (|| -> Result<bool, WalletError> {
        let token = Address::from_str(token_in.trim())
            .map_err(|_| WalletError::InvalidTransaction("dex token in".into()))?;
        let owner = Address::from_str(owner.trim())
            .map_err(|_| WalletError::InvalidTransaction("dex owner".into()))?;
        let router = Address::from_str(router.trim())
            .map_err(|_| WalletError::InvalidTransaction("dex router".into()))?;
        let need = U256::from_str(&amount_in)
            .map_err(|_| WalletError::InvalidAmount("dex amount".into()))?;
        handle.block_on(erc20_allowance_covers(&rpc_url, token, owner, router, need))
    })();
    UiJobResult::DexAllowanceCheck(parsed)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agg_quote(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    venue: AggVenue,
    token_in: String,
    token_out: String,
    amount: String,
    slippage: f64,
    native_in: bool,
    native_out: bool,
    account: Option<String>,
) -> UiJobResult {
    let parsed = parse_agg_quote_request(
        &token_in,
        &token_out,
        &amount,
        slippage,
        native_in,
        native_out,
        account.as_deref(),
    );
    let dir = StateManager::default_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let chain_id = wallet
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .networks()
        .active()
        .chain_id;
    UiJobResult::AggQuote(match parsed {
        Ok(req) => handle.block_on(quote_aggregator(
            venue,
            &req,
            chain_id,
            dir.as_deref(),
            None,
        )),
        Err(e) => Err(e),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agg_compare_quote(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    token_in: String,
    token_out: String,
    amount: String,
    slippage: f64,
    native_in: bool,
    native_out: bool,
    account: Option<String>,
) -> UiJobResult {
    let parsed = parse_agg_quote_request(
        &token_in,
        &token_out,
        &amount,
        slippage,
        native_in,
        native_out,
        account.as_deref(),
    );
    let dir = StateManager::default_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let chain_id = wallet
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .networks()
        .active()
        .chain_id;
    UiJobResult::AggCompareQuote(match parsed {
        Ok(req) => handle.block_on(quote_live_aggregators(&req, chain_id, dir.as_deref(), None)),
        Err(e) => vec![AggQuoteOutcome {
            venue: AggVenue::SquirrelSwap,
            result: Err(e),
        }],
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn dex_quote(
    handle: &Handle,
    quote_gen: u64,
    chain_id: u64,
    rpc_url: String,
    protocol_v2: bool,
    router: String,
    quoter: Option<String>,
    amount_in: String,
    fee: u32,
    path: Vec<String>,
    token_in: String,
    token_out: String,
    wpls: Option<String>,
    native_in: bool,
) -> UiJobResult {
    let parsed = (|| -> Result<DexQuote, WalletError> {
        let amount_in = U256::from_str(&amount_in)
            .map_err(|_| WalletError::InvalidAmount("dex amount".into()))?;
        if protocol_v2 {
            let hops: Result<Vec<Address>, _> = path
                .iter()
                .map(|s| {
                    Address::from_str(s.trim())
                        .map_err(|_| WalletError::InvalidTransaction("dex path token".into()))
                })
                .collect();
            let hops = hops?;
            let router = Address::from_str(&router)
                .map_err(|_| WalletError::InvalidTransaction("dex router".into()))?;
            handle.block_on(quote_v2_exact_in(&rpc_url, router, amount_in, &hops))
        } else {
            let token_in = Address::from_str(token_in.trim())
                .map_err(|_| WalletError::InvalidTransaction("dex token in".into()))?;
            let token_out = Address::from_str(token_out.trim())
                .map_err(|_| WalletError::InvalidTransaction("dex token out".into()))?;
            let wpls = match wpls {
                Some(s) if !s.trim().is_empty() => Some(
                    Address::from_str(s.trim())
                        .map_err(|_| WalletError::InvalidTransaction("dex WPLS".into()))?,
                ),
                _ => None,
            };
            let quoter = match quoter {
                Some(s) if !s.trim().is_empty() => Some(
                    Address::from_str(s.trim())
                        .map_err(|_| WalletError::InvalidTransaction("dex quoter".into()))?,
                ),
                _ => None,
            };
            let (hops, hop_fees, amount_out) =
                if vaughan_core::core::deployment_for_chain(chain_id).is_some() {
                    let route = handle.block_on(discover_v3_swap_route(
                        &rpc_url, chain_id, token_in, token_out, amount_in, wpls, native_in,
                    ))?;
                    (route.path, route.hop_fees, route.amount_out)
                } else {
                    let hops = handle.block_on(resolve_v3_swap_path(
                        &rpc_url, chain_id, token_in, token_out, fee, wpls, native_in,
                    ))?;
                    let quote = handle.block_on(quote_v3_path_exact_in(
                        &rpc_url, chain_id, &hops, amount_in, fee, quoter,
                    ))?;
                    (
                        hops,
                        vec![fee; quote.path.len().saturating_sub(1)],
                        quote.amount_out,
                    )
                };
            Ok(DexQuote {
                amount_out,
                path: hops,
                fee_tier: hop_fees.first().copied().unwrap_or(0),
                hop_fees,
            })
        }
    })();
    UiJobResult::DexQuote {
        quote_gen,
        result: parsed,
    }
}

pub(crate) fn bridge_quote(
    handle: &Handle,
    src_token: String,
    dst_token: String,
    amount: String,
    src_chain: u64,
    dst_chain: u64,
    recipient: String,
) -> UiJobResult {
    let parsed = (|| -> Result<BridgeQuoteRequest, WalletError> {
        let amount = U256::from_str(&amount)
            .map_err(|_| WalletError::InvalidAmount("bridge amount".into()))?;
        let recipient = Address::from_str(&recipient)
            .map_err(|_| WalletError::InvalidTransaction("bridge recipient".into()))?;
        let src_token = if src_token.eq_ignore_ascii_case("USDC") {
            BridgeAsset::Symbol("USDC")
        } else {
            BridgeAsset::Address(
                Address::from_str(&src_token)
                    .map_err(|_| WalletError::InvalidTransaction("bridge src_token".into()))?,
            )
        };
        let dst_token = if dst_token.eq_ignore_ascii_case("USDC") {
            BridgeAsset::Symbol("USDC")
        } else {
            BridgeAsset::Address(
                Address::from_str(&dst_token)
                    .map_err(|_| WalletError::InvalidTransaction("bridge dst_token".into()))?,
            )
        };
        Ok(BridgeQuoteRequest {
            src_token,
            dst_token,
            amount,
            src_chain,
            dst_chain,
            recipient,
        })
    })();
    UiJobResult::BridgeQuote(Box::new(match parsed {
        Ok(req) => match LibertySwapClient::public() {
            Ok(client) => handle.block_on(client.quote(&req)),
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    }))
}
