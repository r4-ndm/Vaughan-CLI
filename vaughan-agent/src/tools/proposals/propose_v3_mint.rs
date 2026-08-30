//! Proposal tool: V3 mint (open concentrated LP) → TxProposal.

use alloy::primitives::U256;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use wiz4rd_math::fee_tiers::tick_spacing;
use wiz4rd_math::nearest_usable_tick;
use wiz4rd_sdk::tx::liquidity::build_mint_tx;
use wiz4rd_sdk::tx::swap::apply_slippage;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::v3_lp::{
    load_lp_pool, proposal_network_id, resolve_lp_venue, venue_param_schema,
};
use crate::tools::wiz4rd_common::resolve_token;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::is_allowed_dex_router;

#[derive(Default)]
pub struct ProposeV3MintTool;

impl ProposeV3MintTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV3MintTool {
    fn name(&self) -> &str {
        "propose_v3_mint"
    }

    fn description(&self) -> &str {
        "Draft a V3 mint (open LP NFT) for Vaughan approval. Prefer get_v3_pool first on 943; \
         on 369 use venue 9mm. Never signs. May need ERC-20 approve to the position manager separately."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_a": {
                    "type": "string",
                    "description": "First token (address, WPLS, WZRD on 943)"
                },
                "token_b": {
                    "type": "string",
                    "description": "Second token"
                },
                "amount_a": {
                    "type": "string",
                    "description": "Desired amount of token_a in raw units"
                },
                "amount_b": {
                    "type": "string",
                    "description": "Desired amount of token_b in raw units"
                },
                "fee": { "type": "integer", "default": 500 },
                "tick_lower": {
                    "type": "integer",
                    "description": "Optional lower tick (else ±range_spacings around current)"
                },
                "tick_upper": {
                    "type": "integer",
                    "description": "Optional upper tick"
                },
                "range_spacings": {
                    "type": "integer",
                    "description": "When ticks omitted: half-width in tick spacings (default 10)",
                    "default": 10
                },
                "slippage_bps": {
                    "type": "integer",
                    "description": "Floor amounts = desired × (1 − bps/10000); default 50",
                    "default": 50
                },
                "venue": venue_param_schema()["venue"],
                "explanation": { "type": "string" }
            },
            "required": ["token_a", "token_b", "amount_a", "amount_b", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let recipient = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall(
                "No active wallet — unlock Vaughan TUI or pass session account".into(),
            )
        })?;
        let token_a_s = args
            .get("token_a")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_a".into()))?;
        let token_b_s = args
            .get("token_b")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_b".into()))?;
        let amount_a = U256::from_str(
            args.get("amount_a")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing amount_a".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount_a: {e}")))?;
        let amount_b = U256::from_str(
            args.get("amount_b")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing amount_b".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount_b: {e}")))?;
        let fee = args.get("fee").and_then(|v| v.as_u64()).unwrap_or(500) as u32;
        let slippage_bps = args
            .get("slippage_bps")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as u32;
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?;
        let venue = resolve_lp_venue(&args, context.chain_id)?;

        let (token_a, _) = resolve_token(token_a_s, context.chain_id)?;
        let (token_b, _) = resolve_token(token_b_s, context.chain_id)?;
        if token_a == token_b {
            return Err(AgentError::InvalidToolCall(
                "token_a and token_b must differ".into(),
            ));
        }

        let (cfg, pool) = load_lp_pool(context, venue, token_a, token_b, fee).await?;

        // Map amounts into pool token0/token1 order.
        let (amount0_desired, amount1_desired) = if token_a == pool.token0 && token_b == pool.token1
        {
            (amount_a, amount_b)
        } else if token_a == pool.token1 && token_b == pool.token0 {
            (amount_b, amount_a)
        } else {
            return Err(AgentError::InvalidToolCall(
                "token_a/token_b do not match pool tokens".into(),
            ));
        };

        let spacing = tick_spacing(fee)
            .ok_or_else(|| AgentError::InvalidToolCall(format!("unsupported fee tier {fee}")))?;

        let (tick_lower, tick_upper) = match (
            args.get("tick_lower").and_then(|v| v.as_i64()),
            args.get("tick_upper").and_then(|v| v.as_i64()),
        ) {
            (Some(lo), Some(hi)) => {
                let lo = lo as i32;
                let hi = hi as i32;
                if lo >= hi {
                    return Err(AgentError::InvalidToolCall(
                        "tick_lower must be < tick_upper".into(),
                    ));
                }
                (lo, hi)
            }
            (None, None) => {
                let half = args
                    .get("range_spacings")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(10) as i32;
                let lo = nearest_usable_tick(pool.tick - half * spacing, spacing);
                let hi = nearest_usable_tick(pool.tick + half * spacing, spacing);
                if lo >= hi {
                    return Err(AgentError::InvalidToolCall(
                        "auto tick range collapsed — widen range_spacings".into(),
                    ));
                }
                (lo, hi)
            }
            _ => {
                return Err(AgentError::InvalidToolCall(
                    "pass both tick_lower and tick_upper, or neither".into(),
                ));
            }
        };

        let amount0_min = apply_slippage(amount0_desired, slippage_bps);
        let amount1_min = apply_slippage(amount1_desired, slippage_bps);

        let deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() + 600)
            .unwrap_or(u64::MAX);

        let tx = build_mint_tx(
            &cfg,
            pool.token0,
            pool.token1,
            fee,
            tick_lower,
            tick_upper,
            amount0_desired,
            amount1_desired,
            amount0_min,
            amount1_min,
            recipient,
            deadline,
        )
        .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;

        let npm = match tx.to {
            Some(alloy::primitives::TxKind::Call(a)) => a,
            _ => cfg.position_manager.ok_or_else(|| {
                AgentError::InvalidToolCall("mint tx missing position_manager".into())
            })?,
        };
        if !is_allowed_dex_router(context.chain_id, npm) {
            return Err(AgentError::InvalidToolCall(format!(
                "position_manager {npm:#x} not allowlisted for chain {}",
                context.chain_id
            )));
        }
        let calldata = tx
            .input
            .into_input()
            .ok_or_else(|| AgentError::InvalidToolCall("mint tx missing calldata".into()))?;

        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("v3_mint_{}", rand_id()),
                ProposalType::ContractCall {
                    target: npm,
                    function_name: Some("mint".into()),
                },
                npm,
                U256::ZERO,
                calldata,
                600_000,
                true,
                format!(
                    "{explanation} [{} mint fee {fee} ticks [{tick_lower},{tick_upper}] \
                     amt0={amount0_desired} amt1={amount1_desired}]",
                    venue.label()
                ),
            )
            .with_chain(context.chain_id, proposal_network_id(context)),
            context,
        )
        .await;

        Ok(serde_json::to_value(&proposal)?)
    }
}
