//! PulseChain swap aggregators — catalog + no-key quote clients.
//!
//! Primary focus: [`squirrelswap`] (Brain at `api.squirrelswap.pro`, no key).
//! Also live: [`pulseswap`], [`crate::core::piteas`], [`empx`]. Others listed for UX.

mod catalog;
mod empx;
mod pulseswap;
mod routers;
mod squirrelswap;
mod types;

pub use catalog::{AggAccess, AggVenue, AGG_VENUES};
pub use empx::{EmpxClient, EMPX_ROUTER_369};
pub use pulseswap::{PulseSwapClient, PULSESWAP_QUOTE_URL};
pub use routers::{assert_agg_exec_targets, is_allowed_agg_router, OFFICIAL_AGG_ROUTERS};
pub use squirrelswap::{SquirrelPreview, SquirrelSwapClient, SQUIRRELSWAP_BRAIN_URL};
pub use types::{AggExecTx, AggQuote, AggQuoteRequest, NativeSentinel};

use secrecy::SecretString;

use crate::core::piteas::{
    load_api_key, load_file_config, MethodParameters, NativeToken, PiteasClient, PiteasFileConfig,
    QuoteRequest,
};
use crate::error::WalletError;

/// Fetch a ready-to-sign quote from a venue that supports no-key (or configured) access.
///
/// `chain_id` is the wallet/session chain — EmpX refuses anything other than Pulse
/// mainnet (369). `piteas_dir` is the Vaughan data dir (optional partner key for Piteas).
pub async fn quote_aggregator(
    venue: AggVenue,
    req: &AggQuoteRequest,
    chain_id: u64,
    piteas_dir: Option<&std::path::Path>,
    vault_password: Option<&SecretString>,
) -> Result<AggQuote, WalletError> {
    match venue.access() {
        AggAccess::LiveNoKey => match venue {
            AggVenue::SquirrelSwap => SquirrelSwapClient::public()?.prepare_swap(req).await,
            AggVenue::PulseSwap => PulseSwapClient::public()?.quote(req).await,
            AggVenue::Piteas => quote_piteas(req, piteas_dir, vault_password).await,
            AggVenue::Empseal => {
                let rpc = std::env::var("VAUGHAN_EMPX_RPC")
                    .unwrap_or_else(|_| "https://rpc.pulsechain.com".into());
                EmpxClient::for_chain(chain_id, &rpc)?.quote(req).await
            }
            _ => Err(WalletError::Other(format!(
                "{} marked live but has no client",
                venue.label()
            ))),
        },
        AggAccess::NeedsApiKey(why) => {
            Err(WalletError::Other(format!("{} — {why}", venue.label())))
        }
        AggAccess::ListedOnly(why) => Err(WalletError::Other(format!("{} — {why}", venue.label()))),
    }
}

async fn quote_piteas(
    req: &AggQuoteRequest,
    dir: Option<&std::path::Path>,
    password: Option<&SecretString>,
) -> Result<AggQuote, WalletError> {
    let cfg = match dir {
        Some(d) => load_file_config(d)?.unwrap_or_default(),
        None => PiteasFileConfig::default(),
    };
    let key = match (dir, password) {
        (Some(d), Some(pw)) => load_api_key(d, pw)?,
        _ => None,
    };
    let client = PiteasClient::from_config(&cfg, key)?;
    let token_in = match req.token_in_is_native {
        true => NativeToken::Pls,
        false => NativeToken::Address(req.token_in),
    };
    let token_out = match req.token_out_is_native {
        true => NativeToken::Pls,
        false => NativeToken::Address(req.token_out),
    };
    let mut q =
        QuoteRequest::new(token_in, token_out, req.amount_in).with_slippage(req.slippage_percent);
    if let Some(account) = req.account {
        q = q.with_account(account);
    }
    let quote = client.quote(&q).await?;
    let to = MethodParameters::router_address()?;
    assert_agg_exec_targets(to, to)?;
    Ok(AggQuote {
        venue: AggVenue::Piteas,
        amount_in: quote.src_amount_u256()?,
        amount_out: quote.dest_amount_u256()?,
        gas_estimate: Some(quote.gas_use_estimate),
        tx: AggExecTx {
            to,
            data: quote.method_parameters.calldata_bytes()?,
            value: quote.method_parameters.value_u256()?,
        },
        spender: to,
    })
}
