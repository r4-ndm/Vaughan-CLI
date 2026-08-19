//! Sensory tool: Dry-run simulate a call or transaction via eth_call without broadcasting.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use url::Url;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

#[derive(Default)]
pub struct SimulateCallTool;

impl SimulateCallTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SimulateCallTool {
    fn name(&self) -> &str {
        "simulate_call"
    }

    fn description(&self) -> &str {
        "Simulate an EVM contract call via read-only eth_call to predict return data or catch reverts before proposing."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Destination contract address"
                },
                "data": {
                    "type": "string",
                    "description": "Hex-encoded calldata (0x...)"
                },
                "value": {
                    "type": "string",
                    "description": "Optional native value in wei",
                    "default": "0"
                }
            },
            "required": ["to", "data"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let to_str = args
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'to' target".to_string()))?;

        let to = Address::from_str(to_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid target address: {e}")))?;

        let data_str = args
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'data' calldata".to_string()))?;

        let data_bytes = hex::decode(data_str.trim_start_matches("0x"))
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid calldata hex: {e}")))?;

        let value_u256 = if let Some(val_str) = args.get("value").and_then(|v| v.as_str()) {
            U256::from_str(val_str).unwrap_or(U256::ZERO)
        } else {
            U256::ZERO
        };

        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
            alloy::providers::RootProvider::new_http(rpc_url);

        let mut tx = alloy::rpc::types::eth::TransactionRequest::default()
            .to(to)
            .input(Bytes::from(data_bytes).into())
            .value(value_u256);

        if let Some(sender) = context.active_address {
            tx = tx.from(sender);
        }

        match provider.call(tx).await {
            Ok(return_data) => Ok(json!({
                "status": "success",
                "reverted": false,
                "return_data_hex": format!("0x{}", hex::encode(&return_data)),
                "return_data_length": return_data.len(),
            })),
            Err(e) => Ok(json!({
                "status": "reverted",
                "reverted": true,
                "revert_reason": e.to_string(),
            })),
        }
    }
}
