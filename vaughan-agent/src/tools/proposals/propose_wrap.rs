//! Proposal tools: wrap / unwrap native ↔ WPLS (WETH9-shaped).

use alloy::primitives::{Address, Bytes, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::wpls_for_chain;

sol! {
    interface IWETH9 {
        function deposit() external payable;
        function withdraw(uint256 wad) external;
    }
}

fn require_amount_explanation(args: &Value) -> Result<(U256, String), AgentError> {
    let amount = U256::from_str(
        args.get("amount")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing amount (wei)".into()))?,
    )
    .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount: {e}")))?;
    if amount.is_zero() {
        return Err(AgentError::InvalidToolCall("amount must be > 0".into()));
    }
    let explanation = args
        .get("explanation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?
        .to_string();
    Ok((amount, explanation))
}

fn wpls_addr(chain_id: u64) -> Result<Address, AgentError> {
    wpls_for_chain(chain_id).ok_or_else(|| {
        AgentError::InvalidToolCall(format!(
            "no WPLS mapping for chain_id {chain_id} — use Pulse 369/943"
        ))
    })
}

#[derive(Default)]
pub struct ProposeWrapTool;

impl ProposeWrapTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeWrapTool {
    fn name(&self) -> &str {
        "propose_wrap"
    }

    fn description(&self) -> &str {
        "Draft a wrap proposal: native PLS/ETH → WPLS via WETH9 deposit(). Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "amount": {
                    "type": "string",
                    "description": "Native amount in wei to wrap"
                },
                "explanation": { "type": "string" }
            },
            "required": ["amount", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let _ = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("wallet_locked: unlock Vaughan or pass session".into())
        })?;
        let (amount, explanation) = require_amount_explanation(&args)?;
        let wpls = wpls_addr(context.chain_id)?;
        let calldata = Bytes::from(IWETH9::depositCall {}.abi_encode());
        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("wrap_{}", rand_id()),
                ProposalType::ContractCall {
                    target: wpls,
                    function_name: Some("deposit".into()),
                },
                wpls,
                amount,
                calldata,
                80_000,
                true,
                format!("{explanation} [wrap {amount} wei → WPLS]"),
            )
            .with_chain(context.chain_id, None),
            context,
        )
        .await;
        Ok(serde_json::to_value(&proposal)?)
    }
}

#[derive(Default)]
pub struct ProposeUnwrapTool;

impl ProposeUnwrapTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeUnwrapTool {
    fn name(&self) -> &str {
        "propose_unwrap"
    }

    fn description(&self) -> &str {
        "Draft an unwrap proposal: WPLS → native PLS via WETH9 withdraw(wad). Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "amount": {
                    "type": "string",
                    "description": "WPLS amount in wei to unwrap"
                },
                "explanation": { "type": "string" }
            },
            "required": ["amount", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let _ = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("wallet_locked: unlock Vaughan or pass session".into())
        })?;
        let (amount, explanation) = require_amount_explanation(&args)?;
        let wpls = wpls_addr(context.chain_id)?;
        let calldata = Bytes::from(IWETH9::withdrawCall { wad: amount }.abi_encode());
        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("unwrap_{}", rand_id()),
                ProposalType::ContractCall {
                    target: wpls,
                    function_name: Some("withdraw".into()),
                },
                wpls,
                U256::ZERO,
                calldata,
                80_000,
                true,
                format!("{explanation} [unwrap {amount} wei WPLS → native]"),
            )
            .with_chain(context.chain_id, None),
            context,
        )
        .await;
        Ok(serde_json::to_value(&proposal)?)
    }
}
