//! Proposal tool: revoke ERC-20 allowance (`approve(spender, 0)`).

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

sol! {
    interface IERC20Approve {
        function approve(address spender, uint256 amount) external returns (bool);
    }
}

#[derive(Default)]
pub struct ProposeRevokeTool;

impl ProposeRevokeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeRevokeTool {
    fn name(&self) -> &str {
        "propose_revoke"
    }

    fn description(&self) -> &str {
        "Draft an ERC-20 revoke proposal: approve(spender, 0). Prefer list_allowances first. Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token": {
                    "type": "string",
                    "description": "ERC-20 token address"
                },
                "spender": {
                    "type": "string",
                    "description": "Spender address to revoke"
                },
                "explanation": { "type": "string" }
            },
            "required": ["token", "spender", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let _ = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("wallet_locked: unlock Vaughan or pass session".into())
        })?;
        let token = Address::from_str(
            args.get("token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token: {e}")))?;
        let spender = Address::from_str(
            args.get("spender")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing spender".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid spender: {e}")))?;
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?;

        let calldata = Bytes::from(
            IERC20Approve::approveCall {
                spender,
                amount: U256::ZERO,
            }
            .abi_encode(),
        );
        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("revoke_{}", rand_id()),
                ProposalType::ContractCall {
                    target: token,
                    function_name: Some("approve".into()),
                },
                token,
                U256::ZERO,
                calldata,
                60_000,
                true,
                format!("{explanation} [revoke {token:#x} → spender {spender:#x}]"),
            )
            .with_chain(context.chain_id, None),
            context,
        )
        .await;
        Ok(serde_json::to_value(&proposal)?)
    }
}
