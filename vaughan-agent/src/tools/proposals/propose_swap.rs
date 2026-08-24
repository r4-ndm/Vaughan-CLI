//! Proposal tool: Draft a Uniswap V2 / PulseX router swap for human confirmation.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use url::Url;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::{Tool, ToolContext};

sol! {
    interface IUniswapV2RouterSwap {
        function swapExactETHForTokens(
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external payable returns (uint256[] memory amounts);

        function swapExactTokensForETH(
            uint256 amountIn,
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);

        function swapExactTokensForTokens(
            uint256 amountIn,
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);
    }
}

#[derive(Default)]
pub struct ProposeSwapTool;

impl ProposeSwapTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeSwapTool {
    fn name(&self) -> &str {
        "propose_swap"
    }

    fn description(&self) -> &str {
        "Draft a DEX token swap proposal on Uniswap V2 or PulseX Router for human confirmation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "router_address": {
                    "type": "string",
                    "description": "Router contract address (e.g. PulseX Router or Uniswap V2 Router)"
                },
                "path": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Token swap route path [token_in, ..., token_out]"
                },
                "amount_in": {
                    "type": "string",
                    "description": "Amount of input asset in raw base units"
                },
                "min_amount_out": {
                    "type": "string",
                    "description": "Minimum acceptable output amount (slippage protection)"
                },
                "is_native_in": {
                    "type": "boolean",
                    "description": "True if swapping native coin (PLS/ETH) into tokens",
                    "default": false
                },
                "explanation": {
                    "type": "string",
                    "description": "Explanation of the trade rationale"
                }
            },
            "required": ["router_address", "path", "amount_in", "min_amount_out", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let router_str = args
            .get("router_address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'router_address'".to_string()))?;

        let router = Address::from_str(router_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid router address: {e}")))?;

        let path_arr = args
            .get("path")
            .and_then(|v| v.as_array())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'path' array".to_string()))?;

        let mut path = Vec::new();
        for item in path_arr {
            let s = item.as_str().ok_or_else(|| {
                AgentError::InvalidToolCall("Invalid address in path".to_string())
            })?;
            path.push(
                Address::from_str(s).map_err(|e| {
                    AgentError::InvalidToolCall(format!("Bad address in path: {e}"))
                })?,
            );
        }

        if path.len() < 2 {
            return Err(AgentError::InvalidToolCall(
                "Swap path must contain at least 2 tokens".to_string(),
            ));
        }

        let amount_in_str = args
            .get("amount_in")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'amount_in'".to_string()))?;
        let amount_in = U256::from_str(amount_in_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount_in: {e}")))?;

        let min_amount_out_str = args
            .get("min_amount_out")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'min_amount_out'".to_string()))?;
        let min_amount_out = U256::from_str(min_amount_out_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid min_amount_out: {e}")))?;

        let is_native_in = args
            .get("is_native_in")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let recipient = context.active_address.unwrap_or(Address::ZERO);
        let deadline = U256::from(u64::MAX);

        let (calldata, value_wei) = if is_native_in {
            let call = IUniswapV2RouterSwap::swapExactETHForTokensCall {
                amountOutMin: min_amount_out,
                path: path.clone(),
                to: recipient,
                deadline,
            };
            (Bytes::from(call.abi_encode()), amount_in)
        } else {
            let call = IUniswapV2RouterSwap::swapExactTokensForTokensCall {
                amountIn: amount_in,
                amountOutMin: min_amount_out,
                path: path.clone(),
                to: recipient,
                deadline,
            };
            (Bytes::from(call.abi_encode()), U256::ZERO)
        };

        // Pre-flight simulation
        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
            alloy::providers::RootProvider::new_http(rpc_url);

        let mut tx = alloy::rpc::types::eth::TransactionRequest::default()
            .to(router)
            .input(calldata.clone().into())
            .value(value_wei);

        if let Some(sender) = context.active_address {
            tx = tx.from(sender);
        }

        let sim_res = provider.call(tx).await;
        let sim_success = sim_res.is_ok();

        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("Swap proposal");

        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("swap_{}", super::propose_transfer::rand_id()),
                ProposalType::DexSwap {
                    router,
                    path,
                    amount_in,
                    min_amount_out,
                },
                router,
                value_wei,
                calldata,
                250_000,
                sim_success,
                explanation,
            )
            .with_chain(context.chain_id, None),
            context,
        )
        .await;

        Ok(serde_json::to_value(&proposal)?)
    }
}
