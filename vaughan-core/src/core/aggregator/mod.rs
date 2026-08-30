//! PulseChain swap aggregators — catalog + no-key quote clients.
//!
//! Primary focus: [`squirrelswap`] (Brain at `api.squirrelswap.pro`, no key).
//! Also live: [`pulseswap`], [`crate::core::piteas`], [`empx`]. Others listed for UX.

mod catalog;
mod empx;
mod nine_mm;
mod pulseswap;
mod routers;
mod squirrelswap;
mod types;

pub use catalog::{AggAccess, AggVenue, AGG_VENUES};
pub use empx::{EmpxClient, EMPX_ROUTER_369};
pub use nine_mm::{NineMmClient, NineMmPreview, NATIVE_EEEE as NINEMM_NATIVE_EEEE, NINEMM_API_URL};
pub use pulseswap::{PulseSwapClient, PULSESWAP_QUOTE_URL};
pub use routers::{assert_agg_exec_targets, is_allowed_agg_router, OFFICIAL_AGG_ROUTERS};
pub use squirrelswap::{SquirrelPreview, SquirrelSwapClient, SQUIRRELSWAP_BRAIN_URL};
pub use types::{AggExecTx, AggQuote, AggQuoteOutcome, AggQuoteRequest, NativeSentinel};

use alloy::primitives::U256;
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
            AggVenue::NineMm9x => NineMmClient::for_chain(chain_id)?.quote(req).await,
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

/// Compare/rank quotes — uses indicative pricing where exec quotes need a funded wallet.
///
/// 9mm 9X uses `/swap/price` (no PLS required). Other live venues use the normal exec path.
pub async fn quote_aggregator_compare(
    venue: AggVenue,
    req: &AggQuoteRequest,
    chain_id: u64,
    piteas_dir: Option<&std::path::Path>,
    vault_password: Option<&SecretString>,
) -> Result<AggQuote, WalletError> {
    if venue == AggVenue::NineMm9x {
        let client = NineMmClient::for_chain(chain_id)?;
        let preview = client.preview_price(req).await?;
        return Ok(NineMmClient::preview_to_agg_quote(req, preview));
    }
    quote_aggregator(venue, req, chain_id, piteas_dir, vault_password).await
}

/// Fetch quotes from every [`AggVenue::is_live`] aggregator in parallel.
pub async fn quote_live_aggregators(
    req: &AggQuoteRequest,
    chain_id: u64,
    piteas_dir: Option<&std::path::Path>,
    vault_password: Option<&SecretString>,
) -> Vec<AggQuoteOutcome> {
    let venues: Vec<AggVenue> = AGG_VENUES.iter().copied().filter(|v| v.is_live()).collect();
    let futures = venues.into_iter().map(|venue| {
        let req = req.clone();
        async move {
            AggQuoteOutcome {
                venue,
                result: quote_aggregator_compare(venue, &req, chain_id, piteas_dir, vault_password)
                    .await,
            }
        }
    });
    futures_util::future::join_all(futures).await
}

/// Indices into `outcomes` with successful quotes, highest `amount_out` first.
pub fn rank_agg_quote_outcomes(outcomes: &[AggQuoteOutcome]) -> Vec<usize> {
    let mut ranked: Vec<(usize, U256)> = outcomes
        .iter()
        .enumerate()
        .filter_map(|(i, o)| o.result.as_ref().ok().map(|q| (i, q.amount_out)))
        .collect();
    ranked.sort_by_key(|b| std::cmp::Reverse(b.1));
    ranked.into_iter().map(|(i, _)| i).collect()
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
        preview_only: false,
    })
}

#[cfg(test)]
mod compare_tests {
    use super::*;
    use alloy::primitives::{Address, Bytes};

    fn dummy_quote(venue: AggVenue, amount_out: u64) -> AggQuote {
        AggQuote {
            venue,
            amount_in: U256::from(1u64),
            amount_out: U256::from(amount_out),
            gas_estimate: None,
            tx: AggExecTx {
                to: Address::ZERO,
                data: Bytes::new(),
                value: U256::ZERO,
            },
            spender: Address::ZERO,
            preview_only: false,
        }
    }

    #[test]
    fn rank_includes_nine_mm_preview_only_quote() {
        let outcomes = vec![
            AggQuoteOutcome {
                venue: AggVenue::SquirrelSwap,
                result: Ok(dummy_quote(AggVenue::SquirrelSwap, 100)),
            },
            AggQuoteOutcome {
                venue: AggVenue::NineMm9x,
                result: Ok(AggQuote {
                    venue: AggVenue::NineMm9x,
                    amount_in: U256::from(1u64),
                    amount_out: U256::from(250),
                    gas_estimate: Some(500_000),
                    tx: AggExecTx {
                        to: Address::ZERO,
                        data: Bytes::new(),
                        value: U256::ZERO,
                    },
                    spender: Address::ZERO,
                    preview_only: true,
                }),
            },
            AggQuoteOutcome {
                venue: AggVenue::Piteas,
                result: Ok(dummy_quote(AggVenue::Piteas, 200)),
            },
        ];
        let ranked = rank_agg_quote_outcomes(&outcomes);
        assert_eq!(ranked.len(), 3);
        assert_eq!(outcomes[ranked[0]].venue, AggVenue::NineMm9x);
        assert_eq!(outcomes[ranked[1]].venue, AggVenue::Piteas);
    }

    #[test]
    fn rank_puts_highest_amount_out_first() {
        let outcomes = vec![
            AggQuoteOutcome {
                venue: AggVenue::SquirrelSwap,
                result: Ok(dummy_quote(AggVenue::SquirrelSwap, 100)),
            },
            AggQuoteOutcome {
                venue: AggVenue::Empseal,
                result: Ok(dummy_quote(AggVenue::Empseal, 300)),
            },
            AggQuoteOutcome {
                venue: AggVenue::PulseSwap,
                result: Err(WalletError::Other("no route".into())),
            },
            AggQuoteOutcome {
                venue: AggVenue::Piteas,
                result: Ok(dummy_quote(AggVenue::Piteas, 200)),
            },
        ];
        let ranked = rank_agg_quote_outcomes(&outcomes);
        assert_eq!(ranked.len(), 3);
        assert_eq!(outcomes[ranked[0]].venue, AggVenue::Empseal);
        assert_eq!(outcomes[ranked[1]].venue, AggVenue::Piteas);
        assert_eq!(outcomes[ranked[2]].venue, AggVenue::SquirrelSwap);
    }
}
