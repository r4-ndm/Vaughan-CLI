//! Sensory tool: discover which fee tier has a V3 pool for a token pair.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::AgentError;
use crate::tools::v3_lp::resolve_lp_venue;
use crate::tools::wiz4rd_common::resolve_token;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{discover_v3_pool_fee_tier, v3_pool_lifecycle};

#[derive(Default)]
pub struct DiscoverV3PoolFeeTool;

impl DiscoverV3PoolFeeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DiscoverV3PoolFeeTool {
    fn name(&self) -> &str {
        "discover_v3_pool_fee"
    }

    fn description(&self) -> &str {
        "Find the first on-chain V3 pool fee tier for a token pair and its lifecycle \
         (missing / uninitialized / ready). Use before LP Brews when fee is unknown."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_a": { "type": "string" },
                "token_b": { "type": "string" },
                "venue": {
                    "type": "string",
                    "description": "wiz4rd (943) or 9mm/9inch (369)"
                }
            },
            "required": ["token_a", "token_b"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let token_a_s = args
            .get("token_a")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_a".into()))?;
        let token_b_s = args
            .get("token_b")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_b".into()))?;
        let venue = resolve_lp_venue(&args, context.chain_id)?;
        let (a, _) = resolve_token(token_a_s, context.chain_id)?;
        let (b, _) = resolve_token(token_b_s, context.chain_id)?;
        let (token0, token1) = if a < b { (a, b) } else { (b, a) };

        let fee =
            discover_v3_pool_fee_tier(&context.rpc_url, venue, context.chain_id, token0, token1)
                .await
                .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;

        let lifecycle = if let Some(f) = fee {
            let lc =
                v3_pool_lifecycle(&context.rpc_url, venue, context.chain_id, token0, token1, f)
                    .await
                    .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
            Some(format!("{lc:?}"))
        } else {
            None
        };

        Ok(json!({
            "fee": fee,
            "lifecycle": lifecycle,
            "venue": venue.label(),
            "token0": format!("{token0:#x}"),
            "token1": format!("{token1:#x}"),
        }))
    }
}
