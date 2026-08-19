//! Sensory tool: Search and enumerate liquidity pairs from a V2 Factory.

use alloy::primitives::Address;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use url::Url;
use vaughan_core::browser::events::PairDiscovery;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

#[derive(Default)]
pub struct SearchPairsTool;

impl SearchPairsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SearchPairsTool {
    fn name(&self) -> &str {
        "search_pairs"
    }

    fn description(&self) -> &str {
        "Enumerate liquidity pair addresses deployed by a Uniswap V2 or PulseX factory contract."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "factory_address": {
                    "type": "string",
                    "description": "Factory contract address (e.g. Uniswap V2 / PulseX factory)"
                },
                "start_index": {
                    "type": "integer",
                    "description": "Zero-based starting index in the allPairs array",
                    "default": 0
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of pairs to retrieve (max 50)",
                    "default": 10
                }
            },
            "required": ["factory_address"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let factory_str = args
            .get("factory_address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'factory_address'".to_string()))?;

        let factory = Address::from_str(factory_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid factory address: {e}")))?;

        let start_index = args
            .get("start_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(50);

        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider = alloy::providers::RootProvider::new_http(rpc_url);

        let total_count = PairDiscovery::get_v2_pairs_count(&provider, factory)
            .await
            .unwrap_or(0);

        let pairs =
            PairDiscovery::fetch_v2_pairs_range(&provider, factory, start_index, limit).await;

        let pair_list: Vec<String> = pairs
            .into_iter()
            .map(|p| p.pair_address.to_string())
            .collect();

        Ok(json!({
            "factory": factory.to_string(),
            "total_pairs_count": total_count,
            "start_index": start_index,
            "count": pair_list.len(),
            "pairs": pair_list,
        }))
    }
}
