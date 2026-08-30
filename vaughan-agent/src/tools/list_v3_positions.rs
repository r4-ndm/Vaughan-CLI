//! Sensory tool: list V3 LP NFTs for an address (catalog venues: wiz4rd 943, 9mm 369).

use alloy::primitives::Address;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::tools::v3_lp::{resolve_lp_venue, venue_param_schema};
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{list_v3_lp_positions, venue_slug};

#[derive(Default)]
pub struct ListV3PositionsTool;

impl ListV3PositionsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ListV3PositionsTool {
    fn name(&self) -> &str {
        "list_v3_positions"
    }

    fn description(&self) -> &str {
        "List V3 LP NFT positions for an address. Optional venue (wiz4rd on 943, 9mm on 369). \
         Optional from_block/to_block to bound log scans."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "account_address": {
                    "type": "string",
                    "description": "Owner (default: unlocked session address)"
                },
                "from_block": { "type": "integer" },
                "to_block": { "type": "integer" },
                "venue": venue_param_schema()["venue"]
            }
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let owner = if let Some(s) = args.get("account_address").and_then(|v| v.as_str()) {
            Address::from_str(s)
                .map_err(|e| AgentError::InvalidToolCall(format!("Invalid account_address: {e}")))?
        } else {
            context.active_address.ok_or_else(|| {
                AgentError::InvalidToolCall(
                    "wallet_locked: unlock Vaughan or pass account_address".into(),
                )
            })?
        };
        let from_block = args.get("from_block").and_then(|v| v.as_u64());
        let to_block = args.get("to_block").and_then(|v| v.as_u64());
        let venue = resolve_lp_venue(&args, context.chain_id)?;

        let positions = list_v3_lp_positions(
            &context.rpc_url,
            venue,
            context.chain_id,
            owner,
            from_block,
            to_block,
        )
        .await
        .map_err(|e| AgentError::ProviderError(format!("list_positions: {}", e.user_message())))?;

        let rows: Vec<_> = positions
            .iter()
            .map(|p| {
                json!({
                    "token_id": p.token_id.to_string(),
                    "token0": format!("{:#x}", p.token0),
                    "token1": format!("{:#x}", p.token1),
                    "fee": p.fee,
                    "tick_lower": p.tick_lower,
                    "tick_upper": p.tick_upper,
                    "liquidity": p.liquidity.to_string(),
                    "tokens_owed0": p.tokens_owed0.to_string(),
                    "tokens_owed1": p.tokens_owed1.to_string(),
                })
            })
            .collect();

        Ok(json!({
            "owner": format!("{owner:#x}"),
            "chain_id": context.chain_id,
            "venue": venue_slug(venue),
            "positions": rows,
        }))
    }
}
