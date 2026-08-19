//! Sensory tool: Get native balance or ERC-20 token balance.

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use url::Url;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

sol! {
    interface IERC20Balance {
        function balanceOf(address account) external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
    }
}

#[derive(Default)]
pub struct GetBalanceTool;

impl GetBalanceTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GetBalanceTool {
    fn name(&self) -> &str {
        "get_balance"
    }

    fn description(&self) -> &str {
        "Query the native coin balance or ERC-20 token balance for an address."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "account_address": {
                    "type": "string",
                    "description": "Account address to query balance for"
                },
                "token_address": {
                    "type": "string",
                    "description": "Optional ERC-20 token contract address (omit for native coin balance)"
                }
            },
            "required": ["account_address"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let account_str = args
            .get("account_address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'account_address'".to_string()))?;

        let account = Address::from_str(account_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid account address: {e}")))?;

        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
            alloy::providers::RootProvider::new_http(rpc_url);

        if let Some(token_str) = args.get("token_address").and_then(|v| v.as_str()) {
            let token_addr = Address::from_str(token_str)
                .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token address: {e}")))?;

            let bal_call = IERC20Balance::balanceOfCall { account };
            let tx = alloy::rpc::types::eth::TransactionRequest::default()
                .to(token_addr)
                .input(bal_call.abi_encode().into());

            let res = provider.call(tx).await.map_err(|e| {
                AgentError::ProviderError(format!("Failed to query ERC-20 balance: {e}"))
            })?;

            let balance = if res.len() >= 32 {
                U256::from_be_slice(&res[..32]).to_string()
            } else {
                "0".to_string()
            };

            Ok(json!({
                "account": account.to_string(),
                "token": token_addr.to_string(),
                "balance_raw": balance,
            }))
        } else {
            let native_balance = provider.get_balance(account).await.map_err(|e| {
                AgentError::ProviderError(format!("Failed to query native balance: {e}"))
            })?;

            Ok(json!({
                "account": account.to_string(),
                "balance_wei": native_balance.to_string(),
            }))
        }
    }
}
