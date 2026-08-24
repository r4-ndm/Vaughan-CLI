//! Sensory tool: offline wiz4rd V3 swap quote from live pool state.

use alloy::primitives::U256;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::tools::wiz4rd_common::{load_pool, quote_pool, resolve_token};
use crate::tools::{Tool, ToolContext};

#[derive(Default)]
pub struct QuoteV3SwapTool;

impl QuoteV3SwapTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for QuoteV3SwapTool {
    fn name(&self) -> &str {
        "quote_v3_swap"
    }

    fn description(&self) -> &str {
        "Quote an exact-in single-hop swap on wiz4rd V3 (local math on live slot0/liquidity). \
         Read-only. Use propose_v3_swap to queue for human approval. Pulse testnet 943."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_in": {
                    "type": "string",
                    "description": "Input token (address, WPLS, WZRD, native/PLS)"
                },
                "token_out": {
                    "type": "string",
                    "description": "Output token (address, WPLS, WZRD, native/PLS)"
                },
                "amount_in": {
                    "type": "string",
                    "description": "Exact input amount in wei / raw units"
                },
                "fee": {
                    "type": "integer",
                    "description": "Pool fee tier",
                    "default": 500
                }
            },
            "required": ["token_in", "token_out", "amount_in"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let token_in_s = args
            .get("token_in")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_in".into()))?;
        let token_out_s = args
            .get("token_out")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_out".into()))?;
        let amount_in = U256::from_str(
            args.get("amount_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing amount_in".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount_in: {e}")))?;
        let fee = args.get("fee").and_then(|v| v.as_u64()).unwrap_or(500) as u32;

        let (token_in, native_in) = resolve_token(token_in_s, context.chain_id)?;
        let (token_out, _) = resolve_token(token_out_s, context.chain_id)?;
        if token_in == token_out {
            return Err(AgentError::InvalidToolCall(
                "token_in and token_out must differ".into(),
            ));
        }

        let (_cfg, pool) = load_pool(context, token_in, token_out, fee).await?;
        let quote = quote_pool(&pool, token_in, amount_in)?;

        Ok(json!({
            "venue": "wiz4rd",
            "chain_id": context.chain_id,
            "pool": format!("{:#x}", pool.pool),
            "token_in": format!("{:#x}", token_in),
            "token_out": format!("{:#x}", token_out),
            "native_in": native_in,
            "fee": fee,
            "amount_in": quote.amount_in.to_string(),
            "amount_out": quote.amount_out.to_string(),
            "tick": pool.tick,
            "note": "Approximate if swap crosses ticks — propose_v3_swap re-simulates at approve",
        }))
    }
}
