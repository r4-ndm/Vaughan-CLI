//! Sensory tool: Get Uniswap V2 / PulseX pair reserves and tokens.

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
    interface IUniswapV2PairView {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    }
}

#[derive(Default)]
pub struct GetDexReservesTool;

impl GetDexReservesTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GetDexReservesTool {
    fn name(&self) -> &str {
        "get_dex_reserves"
    }

    fn description(&self) -> &str {
        "Query the liquidity reserves and token addresses for a Uniswap V2 or PulseX pair contract."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pair_address": {
                    "type": "string",
                    "description": "The 0x-prefixed liquidity pair address"
                }
            },
            "required": ["pair_address"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let pair_str = args
            .get("pair_address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'pair_address'".to_string()))?;

        let pair = Address::from_str(pair_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid pair address: {e}")))?;

        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
            alloy::providers::RootProvider::new_http(rpc_url);

        // 1. token0
        let t0_call = IUniswapV2PairView::token0Call {};
        let tx0 = alloy::rpc::types::eth::TransactionRequest::default()
            .to(pair)
            .input(t0_call.abi_encode().into());
        let res0 = provider.call(tx0).await.map_err(|e| {
            AgentError::ProviderError(format!("Failed to query token0 on pair {pair}: {e}"))
        })?;
        let token0 = if res0.len() >= 32 {
            Address::from_slice(&res0[12..32])
        } else {
            Address::ZERO
        };

        // 2. token1
        let t1_call = IUniswapV2PairView::token1Call {};
        let tx1 = alloy::rpc::types::eth::TransactionRequest::default()
            .to(pair)
            .input(t1_call.abi_encode().into());
        let res1 = provider.call(tx1).await.map_err(|e| {
            AgentError::ProviderError(format!("Failed to query token1 on pair {pair}: {e}"))
        })?;
        let token1 = if res1.len() >= 32 {
            Address::from_slice(&res1[12..32])
        } else {
            Address::ZERO
        };

        // 3. getReserves
        let res_call = IUniswapV2PairView::getReservesCall {};
        let tx_res = alloy::rpc::types::eth::TransactionRequest::default()
            .to(pair)
            .input(res_call.abi_encode().into());
        let res_raw = provider.call(tx_res).await.map_err(|e| {
            AgentError::ProviderError(format!("Failed to query getReserves on pair {pair}: {e}"))
        })?;

        let (reserve0, reserve1, timestamp) = if res_raw.len() >= 96 {
            let r0 = U256::from_be_slice(&res_raw[..32]).to_string();
            let r1 = U256::from_be_slice(&res_raw[32..64]).to_string();
            let ts = U256::from_be_slice(&res_raw[64..96]).to_string();
            (r0, r1, ts)
        } else {
            ("0".to_string(), "0".to_string(), "0".to_string())
        };

        let spot_price = if let (Ok(r0_num), Ok(r1_num)) = (reserve0.parse::<f64>(), reserve1.parse::<f64>()) {
            if r0_num > 0.0 {
                Some(r1_num / r0_num)
            } else {
                None
            }
        } else {
            None
        };

        Ok(json!({
            "pair": pair.to_string(),
            "token0": token0.to_string(),
            "token1": token1.to_string(),
            "reserve0": reserve0,
            "reserve1": reserve1,
            "spot_price_token1_per_token0": spot_price,
            "block_timestamp_last": timestamp,
        }))
    }
}
