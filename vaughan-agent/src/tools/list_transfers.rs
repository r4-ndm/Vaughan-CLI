//! Sensory: ERC-20 transfer history for an address.

use alloy::primitives::Address;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use vaughan_core::chains::evm::adapter::EvmAdapter;
use vaughan_core::chains::evm::networks::get_network_by_chain_id;
use vaughan_core::chains::ChainAdapter;

#[derive(Default)]
pub struct ListTransfersTool;

impl ListTransfersTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ListTransfersTool {
    fn name(&self) -> &str {
        "list_transfers"
    }

    fn description(&self) -> &str {
        "List recent ERC-20 Transfer logs for an address (sent/received). No explorer API."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "account_address": { "type": "string" },
                "limit": { "type": "integer", "default": 25 }
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
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(25) as u32;
        let net = get_network_by_chain_id(context.chain_id);
        let name = net.as_ref().map(|n| n.name.as_str()).unwrap_or("evm");
        let adapter = EvmAdapter::new(&context.rpc_url, context.chain_id, name, &[])
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;
        let rows = adapter
            .get_transaction_history(&format!("{owner:#x}"), limit)
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;
        let out: Vec<_> = rows
            .iter()
            .map(|r| {
                json!({
                    "hash": r.hash,
                    "from": r.from,
                    "to": r.to,
                    "value": r.value,
                    "timestamp": r.timestamp,
                    "status": format!("{:?}", r.status),
                    "token_symbol": r.token_symbol,
                    "token_address": r.token_address,
                    "is_token_transfer": r.is_token_transfer,
                })
            })
            .collect();
        Ok(json!({
            "address": format!("{owner:#x}"),
            "chain_id": context.chain_id,
            "transfers": out,
        }))
    }
}
