//! Catalog-backed V3 LP helpers for MCP / agent tools.
//!
//! Swap and quote tools stay on [`super::wiz4rd_common`] (wiz4rd 943 deploy). LP
//! list/mint/lifecycle tools use [`vaughan_core::core::v3_lp_sdk_config`] so wiz4rd
//! (943) and 9mm (369) share one code path with the TUI.

use alloy::primitives::Address;
use serde_json::{json, Value};
use wiz4rd_sdk::config::Config;
use wiz4rd_sdk::pool::PoolInfo;

use crate::error::AgentError;
use crate::tools::ToolContext;
use vaughan_core::chains::evm::networks::get_network_by_chain_id;
use vaughan_core::core::{
    default_lp_v3_venue, load_v3_lp_pool, lp_v3_venues, parse_dex_venue_label, v3_lp_sdk_config,
    venue_position_manager, venue_slug, DexVenue,
};
use vaughan_core::error::WalletError;

fn map_wallet_err(err: WalletError) -> AgentError {
    AgentError::InvalidToolCall(err.user_message().to_string())
}

fn lp_venue_hints(chain_id: u64) -> String {
    lp_v3_venues(chain_id)
        .map(venue_slug)
        .collect::<Vec<_>>()
        .join(", ")
}

/// JSON-schema fragment for optional `"venue"` on V3 LP tools.
pub fn venue_param_schema() -> Value {
    json!({
        "venue": {
            "type": "string",
            "description": "DEX venue slug (wiz4rd on 943, 9mm on 369). Defaults to the chain default."
        }
    })
}

/// Resolve LP venue from tool args, falling back to [`default_lp_venue`].
pub fn resolve_lp_venue(args: &Value, chain_id: u64) -> Result<DexVenue, AgentError> {
    let venue = if let Some(raw) = args.get("venue").and_then(|v| v.as_str()) {
        parse_dex_venue_label(raw).ok_or_else(|| {
            AgentError::InvalidToolCall(format!(
                "unknown venue {raw:?} — use one of: {}",
                lp_venue_hints(chain_id)
            ))
        })?
    } else {
        default_lp_v3_venue(chain_id).ok_or_else(|| {
            AgentError::InvalidToolCall(format!(
                "no V3 LP venue on chain {chain_id} (wiz4rd 943, 9inch 369; use list_v2_positions for 9inch V2)"
            ))
        })?
    };
    if venue_position_manager(venue, chain_id).is_none() {
        return Err(AgentError::InvalidToolCall(format!(
            "{} has no V3 NPM on chain {chain_id}",
            venue.label()
        )));
    }
    Ok(venue)
}

/// Build wiz4rd-sdk config for a catalogued LP venue on the active chain.
pub fn lp_config(context: &ToolContext, venue: DexVenue) -> Result<Config, AgentError> {
    v3_lp_sdk_config(venue, context.chain_id, &context.rpc_url).map_err(map_wallet_err)
}

/// Network id label for proposals (`pulsechain`, `pulsechain-testnet-v4`, …).
pub fn proposal_network_id(context: &ToolContext) -> Option<String> {
    get_network_by_chain_id(context.chain_id).map(|net| net.id.to_string())
}

/// Load pool state for mint preview on a catalogued LP venue.
pub async fn load_lp_pool(
    context: &ToolContext,
    venue: DexVenue,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> Result<(Config, PoolInfo), AgentError> {
    load_v3_lp_pool(
        &context.rpc_url,
        venue,
        context.chain_id,
        token_a,
        token_b,
        fee,
    )
    .await
    .map_err(|err| match err {
        WalletError::NetworkError(msg) => AgentError::ProviderError(msg),
        other => map_wallet_err(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(chain_id: u64) -> ToolContext {
        ToolContext {
            rpc_url: "http://127.0.0.1:8545".into(),
            chain_id,
            active_address: None,
        }
    }

    #[test]
    fn resolve_default_venue_per_chain() {
        assert_eq!(resolve_lp_venue(&json!({}), 943).unwrap(), DexVenue::Wiz4rd);
        assert_eq!(
            resolve_lp_venue(&json!({}), 369).unwrap(),
            DexVenue::NineInch
        );
        assert!(resolve_lp_venue(&json!({"venue": "wiz4rd"}), 943).is_ok());
        assert!(resolve_lp_venue(&json!({"venue": "9inch"}), 369).is_ok());
    }

    #[test]
    fn reject_nine_mm_on_943() {
        let err = resolve_lp_venue(&json!({"venue": "9mm"}), 943).unwrap_err();
        assert!(err.to_string().contains("943"));
    }

    #[test]
    fn lp_config_wiz4rd_943_only() {
        assert!(lp_config(&ctx(943), DexVenue::Wiz4rd).is_ok());
        assert!(lp_config(&ctx(369), DexVenue::NineInch).is_ok());
        assert!(lp_config(&ctx(369), DexVenue::NineMm).is_ok());
        assert!(lp_config(&ctx(943), DexVenue::NineMm).is_err());
    }

    #[test]
    fn proposal_network_id_matches_chain() {
        assert_eq!(
            proposal_network_id(&ctx(943)).as_deref(),
            Some("pulsechain-testnet-v4")
        );
        assert_eq!(
            proposal_network_id(&ctx(369)).as_deref(),
            Some("pulsechain")
        );
    }
}
