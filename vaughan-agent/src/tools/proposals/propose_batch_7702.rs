//! Proposal tool: Draft a batched EIP-7702 multi-transfer or multicall for human confirmation.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use url::Url;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::{Tool, ToolContext};
use vaughan_aa::abi::Transaction;
use vaughan_aa::encode::encode_execute;

#[derive(Default)]
pub struct ProposeBatch7702Tool;

impl ProposeBatch7702Tool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeBatch7702Tool {
    fn name(&self) -> &str {
        "propose_batch_7702"
    }

    fn description(&self) -> &str {
        "Draft an atomic batched multi-transaction proposal via EIP-7702 smart account delegation for human confirmation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "calls": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "to": { "type": "string" },
                            "value_wei": { "type": "string", "default": "0" },
                            "data": { "type": "string", "default": "0x" }
                        },
                        "required": ["to"]
                    },
                    "description": "List of calls in the batch"
                },
                "explanation": {
                    "type": "string",
                    "description": "Explanation of the batched action"
                }
            },
            "required": ["calls", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let calls_arr = args
            .get("calls")
            .and_then(|v| v.as_array())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'calls' array".to_string()))?;

        let mut txns = Vec::new();
        let mut total_value = U256::ZERO;

        for item in calls_arr {
            let to_str = item.get("to").and_then(|v| v.as_str()).ok_or_else(|| {
                AgentError::InvalidToolCall("Missing 'to' in batch call".to_string())
            })?;
            let to = Address::from_str(to_str).map_err(|e| {
                AgentError::InvalidToolCall(format!("Invalid batch call address: {e}"))
            })?;

            let val_str = item
                .get("value_wei")
                .and_then(|v| v.as_str())
                .unwrap_or("0");
            let value = U256::from_str(val_str).unwrap_or(U256::ZERO);

            let data_str = item.get("data").and_then(|v| v.as_str()).unwrap_or("0x");
            let data_bytes = hex::decode(data_str.trim_start_matches("0x")).unwrap_or_default();

            total_value += value;
            txns.push(Transaction {
                to,
                value,
                data: Bytes::from(data_bytes),
            });
        }

        let batched_calldata = Bytes::from(encode_execute(&txns, &[0u8; 66]));
        let sender = context.active_address.unwrap_or(Address::ZERO);

        // Pre-flight simulation
        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
            alloy::providers::RootProvider::new_http(rpc_url);

        let tx = alloy::rpc::types::eth::TransactionRequest::default()
            .to(sender)
            .input(batched_calldata.clone().into())
            .value(total_value)
            .from(sender);

        let sim_res = provider.call(tx).await;
        let sim_success = sim_res.is_ok();

        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("Batch proposal");

        let proposal = TxProposal::new(
            format!("batch_{}", super::propose_transfer::rand_id()),
            ProposalType::Batch7702 {
                target_count: txns.len(),
                total_value,
            },
            sender,
            total_value,
            batched_calldata,
            180_000,
            sim_success,
            explanation,
        )
        .with_chain(context.chain_id, None);

        Ok(serde_json::to_value(&proposal)?)
    }
}
