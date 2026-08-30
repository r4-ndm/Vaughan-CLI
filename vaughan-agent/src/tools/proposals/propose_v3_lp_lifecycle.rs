//! V3 increase / decrease / collect LP proposals (catalog venues: wiz4rd 943, 9mm 369).

use alloy::primitives::U256;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use wiz4rd_sdk::tx::liquidity::{
    build_collect_tx, build_decrease_liquidity_tx, build_increase_liquidity_tx,
};
use wiz4rd_sdk::tx::swap::apply_slippage;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::v3_lp::{lp_config, proposal_network_id, resolve_lp_venue, venue_param_schema};
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::is_allowed_dex_router;

fn deadline_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() + 600)
        .unwrap_or(u64::MAX)
}

fn require_token_id(args: &Value) -> Result<U256, AgentError> {
    U256::from_str(
        args.get("token_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_id".into()))?,
    )
    .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token_id: {e}")))
}

fn require_explanation(args: &Value) -> Result<&str, AgentError> {
    args.get("explanation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))
}

fn require_active(context: &ToolContext) -> Result<alloy::primitives::Address, AgentError> {
    context.active_address.ok_or_else(|| {
        AgentError::InvalidToolCall(
            "No active wallet — unlock Vaughan TUI or pass session account".into(),
        )
    })
}

fn extract_npm_calldata(
    context: &ToolContext,
    tx: alloy::rpc::types::TransactionRequest,
) -> Result<(alloy::primitives::Address, alloy::primitives::Bytes), AgentError> {
    let npm = match tx.to {
        Some(alloy::primitives::TxKind::Call(a)) => a,
        _ => {
            return Err(AgentError::InvalidToolCall(
                "liquidity tx missing position_manager".into(),
            ))
        }
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
        .ok_or_else(|| AgentError::InvalidToolCall("liquidity tx missing calldata".into()))?;
    Ok((npm, calldata))
}

async fn proposal_from_npm(
    id_prefix: &str,
    npm: alloy::primitives::Address,
    calldata: alloy::primitives::Bytes,
    gas: u64,
    explanation: String,
    context: &ToolContext,
) -> Result<Value, AgentError> {
    let proposal = attach_estimated_fee(
        TxProposal::new(
            format!("{id_prefix}_{}", rand_id()),
            ProposalType::ContractCall {
                target: npm,
                function_name: Some(id_prefix.to_string()),
            },
            npm,
            U256::ZERO,
            calldata,
            gas,
            true,
            explanation,
        )
        .with_chain(context.chain_id, proposal_network_id(context)),
        context,
    )
    .await;
    Ok(serde_json::to_value(&proposal)?)
}

#[derive(Default)]
pub struct ProposeV3IncreaseTool;

impl ProposeV3IncreaseTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV3IncreaseTool {
    fn name(&self) -> &str {
        "propose_v3_increase"
    }

    fn description(&self) -> &str {
        "Draft V3 increaseLiquidity for an existing LP NFT (wiz4rd 943 or 9mm 369). Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_id": { "type": "string" },
                "amount0_desired": { "type": "string" },
                "amount1_desired": { "type": "string" },
                "slippage_bps": { "type": "integer", "default": 50 },
                "venue": venue_param_schema()["venue"],
                "explanation": { "type": "string" }
            },
            "required": ["token_id", "amount0_desired", "amount1_desired", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let _ = require_active(context)?;
        let token_id = require_token_id(&args)?;
        let amount0 = U256::from_str(
            args.get("amount0_desired")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing amount0_desired".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount0_desired: {e}")))?;
        let amount1 = U256::from_str(
            args.get("amount1_desired")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing amount1_desired".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount1_desired: {e}")))?;
        let slippage_bps = args
            .get("slippage_bps")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as u32;
        let explanation = require_explanation(&args)?;
        let venue = resolve_lp_venue(&args, context.chain_id)?;
        let cfg = lp_config(context, venue)?;
        let tx = build_increase_liquidity_tx(
            &cfg,
            token_id,
            amount0,
            amount1,
            apply_slippage(amount0, slippage_bps),
            apply_slippage(amount1, slippage_bps),
            deadline_secs(),
        )
        .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;
        let (npm, calldata) = extract_npm_calldata(context, tx)?;
        proposal_from_npm(
            "increaseLiquidity",
            npm,
            calldata,
            400_000,
            format!(
                "{explanation} [{} increase token_id={token_id} amt0={amount0} amt1={amount1}]",
                venue.label()
            ),
            context,
        )
        .await
    }
}

#[derive(Default)]
pub struct ProposeV3DecreaseTool;

impl ProposeV3DecreaseTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV3DecreaseTool {
    fn name(&self) -> &str {
        "propose_v3_decrease"
    }

    fn description(&self) -> &str {
        "Draft V3 decreaseLiquidity for an LP NFT. Never signs. Follow with propose_v3_collect."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_id": { "type": "string" },
                "liquidity": {
                    "type": "string",
                    "description": "Liquidity units to remove (from list_v3_positions)"
                },
                "amount0_min": { "type": "string", "default": "0" },
                "amount1_min": { "type": "string", "default": "0" },
                "venue": venue_param_schema()["venue"],
                "explanation": { "type": "string" }
            },
            "required": ["token_id", "liquidity", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let _ = require_active(context)?;
        let token_id = require_token_id(&args)?;
        let liquidity: u128 = args
            .get("liquidity")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing liquidity".into()))?
            .parse()
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid liquidity: {e}")))?;
        let amount0_min = args
            .get("amount0_min")
            .and_then(|v| v.as_str())
            .map(U256::from_str)
            .transpose()
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount0_min: {e}")))?
            .unwrap_or(U256::ZERO);
        let amount1_min = args
            .get("amount1_min")
            .and_then(|v| v.as_str())
            .map(U256::from_str)
            .transpose()
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount1_min: {e}")))?
            .unwrap_or(U256::ZERO);
        let explanation = require_explanation(&args)?;
        let venue = resolve_lp_venue(&args, context.chain_id)?;
        let cfg = lp_config(context, venue)?;
        let tx = build_decrease_liquidity_tx(
            &cfg,
            token_id,
            liquidity,
            amount0_min,
            amount1_min,
            deadline_secs(),
        )
        .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;
        let (npm, calldata) = extract_npm_calldata(context, tx)?;
        proposal_from_npm(
            "decreaseLiquidity",
            npm,
            calldata,
            300_000,
            format!(
                "{explanation} [{} decrease token_id={token_id} liquidity={liquidity}]",
                venue.label()
            ),
            context,
        )
        .await
    }
}

#[derive(Default)]
pub struct ProposeV3CollectTool;

impl ProposeV3CollectTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV3CollectTool {
    fn name(&self) -> &str {
        "propose_v3_collect"
    }

    fn description(&self) -> &str {
        "Draft V3 collect (fees + owed tokens) for an LP NFT. Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_id": { "type": "string" },
                "amount0_max": {
                    "type": "string",
                    "description": "Max token0 to collect (default: u128::MAX)"
                },
                "amount1_max": {
                    "type": "string",
                    "description": "Max token1 to collect (default: u128::MAX)"
                },
                "venue": venue_param_schema()["venue"],
                "explanation": { "type": "string" }
            },
            "required": ["token_id", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let recipient = require_active(context)?;
        let token_id = require_token_id(&args)?;
        let amount0_max: u128 = args
            .get("amount0_max")
            .and_then(|v| v.as_str())
            .map(|s| s.parse())
            .transpose()
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount0_max: {e}")))?
            .unwrap_or(u128::MAX);
        let amount1_max: u128 = args
            .get("amount1_max")
            .and_then(|v| v.as_str())
            .map(|s| s.parse())
            .transpose()
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount1_max: {e}")))?
            .unwrap_or(u128::MAX);
        let explanation = require_explanation(&args)?;
        let venue = resolve_lp_venue(&args, context.chain_id)?;
        let cfg = lp_config(context, venue)?;
        let tx = build_collect_tx(&cfg, token_id, recipient, amount0_max, amount1_max)
            .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;
        let (npm, calldata) = extract_npm_calldata(context, tx)?;
        proposal_from_npm(
            "collect",
            npm,
            calldata,
            200_000,
            format!(
                "{explanation} [{} collect token_id={token_id}]",
                venue.label()
            ),
            context,
        )
        .await
    }
}
