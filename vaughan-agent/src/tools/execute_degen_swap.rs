//! Degen-mode write tool: execute a DEX swap via [`DegenTrader`] circuit breakers.
//!
//! Unlike `propose_swap`, this path may sign and broadcast on the isolated
//! burner wallet — only when registered in Degen mode.

use std::str::FromStr;
use std::sync::Arc;

use alloy::primitives::{Address, Bytes, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::degen::DegenTrader;
use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

sol! {
    interface IUniswapV2RouterSwap {
        function swapExactETHForTokens(
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external payable returns (uint256[] memory amounts);

        function swapExactTokensForTokens(
            uint256 amountIn,
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);
    }
}

/// Autonomous swap execution bound to a session [`DegenTrader`].
pub struct ExecuteDegenSwapTool {
    trader: Arc<DegenTrader>,
}

impl ExecuteDegenSwapTool {
    /// Bind this tool to the session burner trader (keys stay inside the trader).
    pub fn new(trader: Arc<DegenTrader>) -> Self {
        Self { trader }
    }
}

#[async_trait]
impl Tool for ExecuteDegenSwapTool {
    fn name(&self) -> &str {
        "execute_degen_swap"
    }

    fn description(&self) -> &str {
        "Degen Bot only: execute a Uniswap V2 / PulseX swap on the burner wallet \
         through Rust circuit breakers (position size, slippage, gas, quorum). \
         Prefer after get_balance / search_pairs / get_dex_reserves. \
         Returns tx_hash (or dry_run=true when VAUGHAN_DEGEN_DRY_RUN is set)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "router_address": {
                    "type": "string",
                    "description": "Router contract (PulseX / Uniswap V2)"
                },
                "path": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Swap path [token_in, ..., token_out]"
                },
                "amount_in": {
                    "type": "string",
                    "description": "Input amount in raw base units (wei)"
                },
                "min_amount_out": {
                    "type": "string",
                    "description": "Minimum output (slippage protection)"
                },
                "is_native_in": {
                    "type": "boolean",
                    "description": "True when spending native PLS/ETH",
                    "default": true
                },
                "slippage_bps": {
                    "type": "integer",
                    "description": "Requested slippage in basis points (max 100 = 1%)",
                    "default": 100
                },
                "pair_address": {
                    "type": "string",
                    "description": "Optional pair for multi-RPC quorum on reserves"
                },
                "rationale": {
                    "type": "string",
                    "description": "Short why this trade is within risk limits"
                }
            },
            "required": ["router_address", "path", "amount_in", "min_amount_out", "rationale"]
        })
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<Value, AgentError> {
        let router_str = args
            .get("router_address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'router_address'".into()))?;
        let router = Address::from_str(router_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid router: {e}")))?;

        let path_arr = args
            .get("path")
            .and_then(|v| v.as_array())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'path'".into()))?;
        let mut path = Vec::new();
        for item in path_arr {
            let s = item
                .as_str()
                .ok_or_else(|| AgentError::InvalidToolCall("Bad path entry".into()))?;
            path.push(
                Address::from_str(s)
                    .map_err(|e| AgentError::InvalidToolCall(format!("Bad path address: {e}")))?,
            );
        }
        if path.len() < 2 {
            return Err(AgentError::InvalidToolCall(
                "Swap path needs at least 2 tokens".into(),
            ));
        }

        let amount_in = U256::from_str(
            args.get("amount_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing 'amount_in'".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount_in: {e}")))?;

        let min_amount_out = U256::from_str(
            args.get("min_amount_out")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing 'min_amount_out'".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid min_amount_out: {e}")))?;

        let is_native_in = args
            .get("is_native_in")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let slippage_bps = args
            .get("slippage_bps")
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as u32;

        let pair = args
            .get("pair_address")
            .and_then(|v| v.as_str())
            .and_then(|s| Address::from_str(s).ok());

        let recipient = self.trader.address();
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

        let outcome = self
            .trader
            .execute_swap(router, pair, calldata, value_wei, amount_in, slippage_bps)
            .await?;

        let rationale = args.get("rationale").and_then(|v| v.as_str()).unwrap_or("");

        Ok(json!({
            "ok": true,
            "dry_run": outcome.dry_run,
            "tx_hash": format!("{:#x}", outcome.tx_hash),
            "burner": format!("{recipient:#x}"),
            "router": format!("{router:#x}"),
            "amount_in": amount_in.to_string(),
            "min_amount_out": min_amount_out.to_string(),
            "slippage_bps": slippage_bps,
            "breaker_tripped": self.trader.circuit_breaker().is_tripped(),
            "rationale": rationale,
        }))
    }
}
