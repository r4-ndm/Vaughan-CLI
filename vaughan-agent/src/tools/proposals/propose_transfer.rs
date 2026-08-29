//! Proposal tool: Draft a native coin or ERC-20 transfer for human confirmation.

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
    interface IERC20Transfer {
        function transfer(address to, uint256 amount) external returns (bool);
    }
}

#[derive(Default)]
pub struct ProposeTransferTool;

impl ProposeTransferTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeTransferTool {
    fn name(&self) -> &str {
        "propose_transfer"
    }

    fn description(&self) -> &str {
        "Draft a native coin or ERC-20 token transfer proposal for human confirmation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "recipient": {
                    "type": "string",
                    "description": "Recipient address (0x...)"
                },
                "amount": {
                    "type": "string",
                    "description": "Amount in raw base units (e.g. wei for native coin)"
                },
                "token_address": {
                    "type": "string",
                    "description": "Optional ERC-20 token contract address (omit for native coin transfer)"
                },
                "explanation": {
                    "type": "string",
                    "description": "Short explanation of why this transfer is being proposed"
                }
            },
            "required": ["recipient", "amount", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let recipient_str = args
            .get("recipient")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'recipient'".to_string()))?;

        let recipient = Address::from_str(recipient_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid recipient address: {e}")))?;

        let amount_str = args
            .get("amount")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'amount'".to_string()))?;

        let amount = U256::from_str(amount_str)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount integer: {e}")))?;

        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("Transfer proposal")
            .to_string();

        let (target, value, calldata, proposal_type) = if let Some(token_str) =
            args.get("token_address").and_then(|v| v.as_str())
        {
            let token = Address::from_str(token_str)
                .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token address: {e}")))?;
            let call = IERC20Transfer::transferCall {
                to: recipient,
                amount,
            };
            (
                token,
                U256::ZERO,
                Bytes::from(call.abi_encode()),
                ProposalType::Erc20Transfer {
                    token,
                    recipient,
                    amount,
                },
            )
        } else {
            (
                recipient,
                amount,
                Bytes::new(),
                ProposalType::NativeTransfer {
                    to: recipient,
                    amount_wei: amount,
                },
            )
        };

        // Pre-flight simulation
        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
            alloy::providers::RootProvider::new_http(rpc_url);

        let mut tx = alloy::rpc::types::eth::TransactionRequest::default()
            .to(target)
            .input(calldata.clone().into())
            .value(value);

        if let Some(sender) = context.active_address {
            tx = tx.from(sender);
        }

        let sim_res = provider.call(tx).await;
        let sim_success = sim_res.is_ok();

        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("prop_{}", rand_id()),
                proposal_type,
                target,
                value,
                calldata,
                65_000,
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

/// Time-millis + random suffix: time alone repeats every 100 s, well inside
/// the 600 s pending TTL, so same-tool proposals could otherwise collide.
pub fn rand_id() -> u32 {
    use rand::RngCore;
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        % 100_000) as u32;
    let random = rand::rngs::OsRng.next_u32() % 1000;
    millis * 1000 + random
}
