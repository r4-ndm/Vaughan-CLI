//! V3 pool deployment proposals — `createPool` + `initialize` (wiz4rd 943, 9inch 369).

use alloy::primitives::{Address, Bytes, U256};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::v3_lp::{proposal_network_id, resolve_lp_venue, venue_param_schema};
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{
    build_v3_create_pool_evm, build_v3_initialize_pool_from_tick_evm, is_allowed_dex_router,
    v3_pool_lifecycle, venue_v3_factory, V3PoolLifecycle,
};

fn require_explanation(args: &Value) -> Result<&str, AgentError> {
    args.get("explanation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))
}

fn parse_token(raw: &str, label: &str) -> Result<Address, AgentError> {
    Address::from_str(raw.trim()).map_err(|e| AgentError::InvalidToolCall(format!("{label}: {e}")))
}

#[derive(Default)]
pub struct ProposeV3CreatePoolTool;

impl ProposeV3CreatePoolTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV3CreatePoolTool {
    fn name(&self) -> &str {
        "propose_v3_create_pool"
    }

    fn description(&self) -> &str {
        "Draft V3 createPool on a catalogued factory (wiz4rd 943, 9inch 369). Never signs. \
         Follow with propose_v3_initialize_pool then propose_v3_mint."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_a": { "type": "string", "description": "First token address" },
                "token_b": { "type": "string", "description": "Second token address" },
                "fee": {
                    "type": "integer",
                    "description": "Fee tier (500=0.05%, 2500=0.25%, 10000=1%, 20000=2%)",
                    "default": 500
                },
                "venue": venue_param_schema()["venue"],
                "explanation": { "type": "string" }
            },
            "required": ["token_a", "token_b", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let from = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall(
                "No active wallet — unlock Vaughan TUI or pass session account".into(),
            )
        })?;
        let token_a = parse_token(
            args.get("token_a")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token_a".into()))?,
            "token_a",
        )?;
        let token_b = parse_token(
            args.get("token_b")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token_b".into()))?,
            "token_b",
        )?;
        if token_a == token_b {
            return Err(AgentError::InvalidToolCall(
                "token_a and token_b must differ".into(),
            ));
        }
        let fee = args.get("fee").and_then(|v| v.as_u64()).unwrap_or(500) as u32;
        let explanation = require_explanation(&args)?;
        let venue = resolve_lp_venue(&args, context.chain_id)?;
        let lifecycle = v3_pool_lifecycle(
            &context.rpc_url,
            venue,
            context.chain_id,
            token_a,
            token_b,
            fee,
        )
        .await
        .map_err(|e| AgentError::ProviderError(e.user_message()))?;
        if lifecycle != V3PoolLifecycle::Missing {
            return Err(AgentError::InvalidToolCall(format!(
                "pool already exists ({lifecycle:?}) — use propose_v3_initialize_pool or propose_v3_mint"
            )));
        }
        let factory = venue_v3_factory(venue, context.chain_id).ok_or_else(|| {
            AgentError::InvalidToolCall(format!("{} has no V3 factory", venue.label()))
        })?;
        if !is_allowed_dex_router(context.chain_id, factory) {
            return Err(AgentError::InvalidToolCall(format!(
                "factory {factory:#x} not allowlisted on chain {}",
                context.chain_id
            )));
        }
        let evm = build_v3_create_pool_evm(
            &format!("{from:#x}"),
            venue,
            context.chain_id,
            &context.rpc_url,
            token_a,
            token_b,
            fee,
        )
        .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        let calldata = hex::decode(evm.data.as_deref().unwrap_or("0x").trim_start_matches("0x"))
            .map_err(|e| AgentError::InvalidToolCall(format!("calldata: {e}")))?;
        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("v3create_{}", rand_id()),
                ProposalType::ContractCall {
                    target: factory,
                    function_name: Some("createPool".into()),
                },
                factory,
                U256::ZERO,
                Bytes::from(calldata),
                400_000,
                true,
                format!(
                    "{explanation} [{} createPool fee {fee} {token_a:#x}/{token_b:#x}]",
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

#[derive(Default)]
pub struct ProposeV3InitializePoolTool;

impl ProposeV3InitializePoolTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV3InitializePoolTool {
    fn name(&self) -> &str {
        "propose_v3_initialize_pool"
    }

    fn description(&self) -> &str {
        "Draft V3 initialize(sqrtPriceX96) on an existing uninitialized pool. \
         Use initial_tick 0 for 1:1 starting price (same decimals). Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_a": { "type": "string" },
                "token_b": { "type": "string" },
                "fee": { "type": "integer", "default": 500 },
                "initial_tick": {
                    "type": "integer",
                    "description": "Starting price tick (0 = 1:1). Adjust for decimal offset.",
                    "default": 0
                },
                "venue": venue_param_schema()["venue"],
                "explanation": { "type": "string" }
            },
            "required": ["token_a", "token_b", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let from = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("No active wallet — unlock Vaughan TUI".into())
        })?;
        let token_a = parse_token(
            args.get("token_a")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token_a".into()))?,
            "token_a",
        )?;
        let token_b = parse_token(
            args.get("token_b")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token_b".into()))?,
            "token_b",
        )?;
        let fee = args.get("fee").and_then(|v| v.as_u64()).unwrap_or(500) as u32;
        let initial_tick = args
            .get("initial_tick")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;
        let explanation = require_explanation(&args)?;
        let venue = resolve_lp_venue(&args, context.chain_id)?;
        let lifecycle = v3_pool_lifecycle(
            &context.rpc_url,
            venue,
            context.chain_id,
            token_a,
            token_b,
            fee,
        )
        .await
        .map_err(|e| AgentError::ProviderError(e.user_message()))?;
        let pool = match lifecycle {
            V3PoolLifecycle::Uninitialized { pool } => pool,
            V3PoolLifecycle::Missing => {
                return Err(AgentError::InvalidToolCall(
                    "pool missing — propose_v3_create_pool first".into(),
                ))
            }
            V3PoolLifecycle::Ready => {
                return Err(AgentError::InvalidToolCall(
                    "pool already initialized — use propose_v3_mint".into(),
                ))
            }
        };
        let evm = build_v3_initialize_pool_from_tick_evm(
            &format!("{from:#x}"),
            context.chain_id,
            pool,
            initial_tick,
        )
        .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        let calldata = hex::decode(evm.data.as_deref().unwrap_or("0x").trim_start_matches("0x"))
            .map_err(|e| AgentError::InvalidToolCall(format!("calldata: {e}")))?;
        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("v3init_{}", rand_id()),
                ProposalType::ContractCall {
                    target: pool,
                    function_name: Some("initialize".into()),
                },
                pool,
                U256::ZERO,
                Bytes::from(calldata),
                200_000,
                true,
                format!(
                    "{explanation} [{} initialize fee {fee} tick {initial_tick} pool {pool:#x}]",
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
