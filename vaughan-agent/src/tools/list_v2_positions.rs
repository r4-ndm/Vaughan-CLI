//! Sensory tool: list 9inch V2 LP pair balances for an address.

use alloy::primitives::Address;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{default_v2_watch_pairs, list_v2_lp_positions, lp_v2_venue, venue_slug};

#[derive(Default)]
pub struct ListV2PositionsTool;

impl ListV2PositionsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ListV2PositionsTool {
    fn name(&self) -> &str {
        "list_v2_positions"
    }

    fn description(&self) -> &str {
        "List Uniswap V2–style LP positions (pair LP token balances). 9inch on Pulse mainnet (369). \
         Probes default WPLS pairs plus optional token0/token1."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "account_address": {
                    "type": "string",
                    "description": "Owner (default: unlocked session address)"
                },
                "token0": { "type": "string", "description": "Optional extra pair leg" },
                "token1": { "type": "string", "description": "Optional extra pair leg" }
            }
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        if context.chain_id != 369 {
            return Err(AgentError::InvalidToolCall(
                "list_v2_positions is 9inch mainnet (369) only — switch network".into(),
            ));
        }
        let venue = lp_v2_venue(context.chain_id)
            .ok_or_else(|| AgentError::InvalidToolCall("no V2 LP venue on this chain".into()))?;
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
        let mut watch = default_v2_watch_pairs(context.chain_id, venue);
        if let (Some(a), Some(b)) = (
            args.get("token0").and_then(|v| v.as_str()),
            args.get("token1").and_then(|v| v.as_str()),
        ) {
            let ta = Address::from_str(a)
                .map_err(|e| AgentError::InvalidToolCall(format!("token0: {e}")))?;
            let tb = Address::from_str(b)
                .map_err(|e| AgentError::InvalidToolCall(format!("token1: {e}")))?;
            watch.push(if ta < tb { (ta, tb) } else { (tb, ta) });
        }
        let positions =
            list_v2_lp_positions(&context.rpc_url, venue, context.chain_id, owner, &watch)
                .await
                .map_err(|e| AgentError::ProviderError(e.user_message()))?;
        let rows: Vec<_> = positions
            .iter()
            .map(|p| {
                json!({
                    "venue": venue_slug(venue),
                    "pair": format!("{:#x}", p.pair),
                    "token0": format!("{:#x}", p.token0),
                    "token1": format!("{:#x}", p.token1),
                    "lp_balance": p.lp_balance.to_string(),
                    "reserve0": p.reserve0.to_string(),
                    "reserve1": p.reserve1.to_string(),
                    "total_supply": p.total_supply.to_string(),
                    "pool_share_bps": p.pool_share_bps(),
                    "amount0": p.underlying_amounts().0.to_string(),
                    "amount1": p.underlying_amounts().1.to_string(),
                })
            })
            .collect();
        Ok(json!({ "positions": rows, "count": rows.len() }))
    }
}
