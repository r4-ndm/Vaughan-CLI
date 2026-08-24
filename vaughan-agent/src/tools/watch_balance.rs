//! Read helper: balance snapshot for sentient watch loops.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use alloy::primitives::{Address, U256};
use vaughan_core::chains::evm::adapter::EvmAdapter;
use vaughan_core::chains::evm::networks::get_network_by_chain_id;
use vaughan_core::chains::ChainAdapter;

#[derive(Default)]
pub struct WatchBalanceTool;

impl WatchBalanceTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WatchBalanceTool {
    fn name(&self) -> &str {
        "watch_balance"
    }

    fn description(&self) -> &str {
        "Snapshot native balance (and optional ERC-20) for agent watch loops. \
         Optional min_wei / max_wei flags whether threshold crossed."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "account_address": { "type": "string" },
                "token": { "type": "string", "description": "Optional ERC-20; omit for native" },
                "min_wei": { "type": "string" },
                "max_wei": { "type": "string" }
            }
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let owner = if let Some(s) = args.get("account_address").and_then(|v| v.as_str()) {
            Address::from_str(s)
                .map_err(|e| AgentError::InvalidToolCall(format!("Invalid account_address: {e}")))?
        } else {
            context.active_address.ok_or_else(|| {
                AgentError::InvalidToolCall(
                    "wallet_locked: unlock Vaughan or pass account_address".into(),
                )
            })?
        };
        let net = get_network_by_chain_id(context.chain_id);
        let name = net.as_ref().map(|n| n.name.as_str()).unwrap_or("evm");
        let adapter = EvmAdapter::new(&context.rpc_url, context.chain_id, name, &[])
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;

        let (raw, symbol) = if let Some(tok) = args.get("token").and_then(|v| v.as_str()) {
            let bal = adapter
                .get_token_balance(tok, &format!("{owner:#x}"))
                .await
                .map_err(|e| AgentError::ProviderError(e.to_string()))?;
            (bal.raw, bal.token.symbol)
        } else {
            let bal = adapter
                .get_balance(&format!("{owner:#x}"))
                .await
                .map_err(|e| AgentError::ProviderError(e.to_string()))?;
            (bal.raw, bal.token.symbol)
        };
        let amount = U256::from_str(&raw).unwrap_or(U256::ZERO);
        let mut below_min = false;
        let mut above_max = false;
        if let Some(m) = args.get("min_wei").and_then(|v| v.as_str()) {
            if let Ok(min) = U256::from_str(m) {
                below_min = amount < min;
            }
        }
        if let Some(m) = args.get("max_wei").and_then(|v| v.as_str()) {
            if let Ok(max) = U256::from_str(m) {
                above_max = amount > max;
            }
        }
        Ok(json!({
            "address": format!("{owner:#x}"),
            "symbol": symbol,
            "raw": raw,
            "below_min": below_min,
            "above_max": above_max,
            "alert": below_min || above_max,
        }))
    }
}
