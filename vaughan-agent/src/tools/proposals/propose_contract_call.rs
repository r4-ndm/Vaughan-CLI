//! Proposal tool: Draft arbitrary contract calls for human confirmation.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use url::Url;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::{Tool, ToolContext};

#[derive(Default)]
pub struct ProposeContractCallTool;

impl ProposeContractCallTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeContractCallTool {
    fn name(&self) -> &str {
        "propose_contract_call"
    }

    fn description(&self) -> &str {
        "Draft an arbitrary smart contract interaction proposal for human confirmation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Target smart contract address (0x...)"
                },
                "calldata": {
                    "type": "string",
                    "description": "Hex-encoded calldata (0x...)"
                },
                "value_wei": {
                    "type": "string",
                    "description": "Optional native value in wei",
                    "default": "0"
                },
                "function_name": {
                    "type": "string",
                    "description": "Optional human-readable function name"
                },
                "explanation": {
                    "type": "string",
                    "description": "Explanation of the contract call purpose"
                }
            },
            "required": ["to", "calldata", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let to_str = args
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'to'".to_string()))?;

        let to = Address::from_str(to_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid target address: {e}")))?;

        let data_str = args
            .get("calldata")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'calldata'".to_string()))?;

        let data_bytes = hex::decode(data_str.trim_start_matches("0x"))
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid calldata hex: {e}")))?;

        let value_wei = if let Some(val_str) = args.get("value_wei").and_then(|v| v.as_str()) {
            U256::from_str(val_str).unwrap_or(U256::ZERO)
        } else {
            U256::ZERO
        };

        let function_name = args
            .get("function_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("Contract call proposal");

        let calldata = Bytes::from(data_bytes);

        // Pre-flight simulation
        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
            alloy::providers::RootProvider::new_http(rpc_url);

        let mut tx = alloy::rpc::types::eth::TransactionRequest::default()
            .to(to)
            .input(calldata.clone().into())
            .value(value_wei);

        if let Some(sender) = context.active_address {
            tx = tx.from(sender);
        }

        let sim_res = provider.call(tx).await;
        let sim_success = sim_res.is_ok();

        let proposal = TxProposal::new(
            format!("call_{}", super::propose_transfer::rand_id()),
            ProposalType::ContractCall {
                target: to,
                function_name,
            },
            to,
            value_wei,
            calldata,
            120_000,
            sim_success,
            explanation,
        )
        .with_chain(context.chain_id, None);

        Ok(serde_json::to_value(&proposal)?)
    }
}
