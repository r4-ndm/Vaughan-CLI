//! Sensory tool: Inspect smart contract capabilities, ABI, and candidate selectors.

use alloy::primitives::Address;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use url::Url;
use vaughan_core::browser::abi::AbiResolution;
use vaughan_core::browser::BrowserEngine;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

/// Max bytecode selectors returned to the LLM (full count is still reported).
const MAX_SELECTORS_IN_RESULT: usize = 24;

pub struct InspectContractTool {
    engine: BrowserEngine,
}

impl Default for InspectContractTool {
    fn default() -> Self {
        Self::new()
    }
}

impl InspectContractTool {
    pub fn new() -> Self {
        Self {
            engine: BrowserEngine::new(),
        }
    }
}

#[async_trait]
impl Tool for InspectContractTool {
    fn name(&self) -> &str {
        "inspect_contract"
    }

    fn description(&self) -> &str {
        "Inspect a smart contract to detect its standard (ERC-20, Uniswap V2/V3, WETH, Multicall3), verified functions, or candidate bytecode selectors. Returns a compact summary (selector list is capped)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "address": {
                    "type": "string",
                    "description": "The 0x-prefixed contract address to inspect"
                }
            },
            "required": ["address"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let addr_str = args
            .get("address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::InvalidToolCall("Missing 'address' parameter".to_string())
            })?;

        let address = Address::from_str(addr_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid Ethereum address: {e}")))?;

        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider = alloy::providers::RootProvider::new_http(rpc_url);
        let inspection = self
            .engine
            .inspect(&provider, context.chain_id, address)
            .await;

        let verified_functions = match &inspection.abi_resolution {
            AbiResolution::Verified(abi) => abi.functions.keys().cloned().collect::<Vec<String>>(),
            _ => Vec::new(),
        };

        let all_selectors: Vec<String> = inspection
            .candidate_selectors
            .iter()
            .map(|s| format!("0x{}", hex::encode(s)))
            .collect();
        let selector_count = all_selectors.len();
        let candidate_selectors: Vec<String> = all_selectors
            .into_iter()
            .take(MAX_SELECTORS_IN_RESULT)
            .collect();

        let fingerprint_json = serde_json::to_value(&inspection.fingerprint).unwrap_or(json!({}));

        Ok(json!({
            "address": inspection.address.to_string(),
            "chain_id": inspection.chain_id,
            "fingerprint": fingerprint_json,
            "verified_functions": verified_functions,
            "candidate_selector_count": selector_count,
            "candidate_selectors": candidate_selectors,
            "candidate_selectors_truncated": selector_count > MAX_SELECTORS_IN_RESULT,
        }))
    }
}
