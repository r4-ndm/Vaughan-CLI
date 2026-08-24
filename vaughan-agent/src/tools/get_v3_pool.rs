//! Sensory tool: live wiz4rd V3 pool state (slot0 + liquidity).

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::AgentError;
use crate::tools::wiz4rd_common::{load_pool, resolve_token};
use crate::tools::{Tool, ToolContext};

#[derive(Default)]
pub struct GetV3PoolTool;

impl GetV3PoolTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GetV3PoolTool {
    fn name(&self) -> &str {
        "get_v3_pool"
    }

    fn description(&self) -> &str {
        "Read wiz4rd (Pancake V3) pool state on PulseChain: address, slot0 tick/price, liquidity. \
         Testnet 943 only. Tokens: address, WPLS, WZRD, or native/PLS."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_a": {
                    "type": "string",
                    "description": "First token (address, WPLS, WZRD, native)"
                },
                "token_b": {
                    "type": "string",
                    "description": "Second token (address, WPLS, WZRD, native)"
                },
                "fee": {
                    "type": "integer",
                    "description": "Fee tier: 100, 500, 2500, 10000, or 20000",
                    "default": 500
                }
            },
            "required": ["token_a", "token_b"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let token_a = args
            .get("token_a")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_a".into()))?;
        let token_b = args
            .get("token_b")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_b".into()))?;
        let fee = args
            .get("fee")
            .and_then(|v| v.as_u64())
            .unwrap_or(500) as u32;

        let (a, _) = resolve_token(token_a, context.chain_id)?;
        let (b, _) = resolve_token(token_b, context.chain_id)?;
        if a == b {
            return Err(AgentError::InvalidToolCall(
                "token_a and token_b must differ".into(),
            ));
        }

        let (_cfg, pool) = load_pool(context, a, b, fee).await?;

        Ok(json!({
            "venue": "wiz4rd",
            "chain_id": context.chain_id,
            "pool": format!("{:#x}", pool.pool),
            "token0": format!("{:#x}", pool.token0),
            "token1": format!("{:#x}", pool.token1),
            "fee": pool.fee,
            "tick": pool.tick,
            "sqrt_price_x96": pool.sqrt_price_x96.to_string(),
            "price_token1_per_token0_approx": pool.price_f64(),
            "liquidity": pool.liquidity.to_string(),
        }))
    }
}
