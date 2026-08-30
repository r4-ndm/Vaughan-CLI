//! Proposal tool: deploy a fixed-supply testnet ERC-20 (meme-coin launcher).

use alloy::primitives::{Address, Bytes, U256};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{
    encode_erc20_deploy_calldata, parse_token_supply_human, token_launch_allowed,
    validate_token_name, validate_token_symbol,
};

/// Contract creation is heavy; pinned bytecode + constructor args on 943.
const TOKEN_DEPLOY_GAS: u64 = 3_000_000;

#[derive(Default)]
pub struct ProposeTokenLaunchTool;

impl ProposeTokenLaunchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeTokenLaunchTool {
    fn name(&self) -> &str {
        "propose_token_launch"
    }

    fn description(&self) -> &str {
        "Draft a fixed-supply ERC-20 token deploy for Vaughan TUI approval. Testnet only \
         (943 / 31337). Mints full supply to the active wallet and auto-imports after approve. \
         Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Token name (1–32 chars)"
                },
                "symbol": {
                    "type": "string",
                    "description": "Ticker / symbol (1–11 alphanumeric)"
                },
                "supply": {
                    "type": "string",
                    "description": "Fixed supply in human units (18 decimals), e.g. \"1000000\""
                },
                "explanation": {
                    "type": "string",
                    "description": "Why the agent is proposing this launch"
                }
            },
            "required": ["name", "symbol", "supply", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        if !token_launch_allowed(context.chain_id) {
            return Err(AgentError::InvalidToolCall(format!(
                "token launch is testnet-only (chain {}); use Pulse testnet 943",
                context.chain_id
            )));
        }

        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing name".into()))?;
        let symbol = args
            .get("symbol")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing symbol".into()))?;
        let supply = args
            .get("supply")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing supply".into()))?;
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("Token launch proposal");

        let name =
            validate_token_name(name).map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;
        let symbol = validate_token_symbol(symbol)
            .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;
        let supply_raw = parse_token_supply_human(supply)
            .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;

        let recipient = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall(
                "wallet_locked: unlock Vaughan TUI so the agent knows the mint recipient".into(),
            )
        })?;

        let calldata = encode_erc20_deploy_calldata(&name, &symbol, supply_raw, recipient)
            .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;

        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("launch_{}", rand_id()),
                ProposalType::TokenLaunch {
                    name: name.clone(),
                    symbol: symbol.clone(),
                    supply_human: supply.trim().to_string(),
                },
                Address::ZERO,
                U256::ZERO,
                Bytes::from(calldata),
                TOKEN_DEPLOY_GAS,
                true,
                format!(
                    "{explanation} — deploy {symbol} ({name}), supply {supply} (18 decimals) to active wallet"
                ),
            )
            .with_chain(context.chain_id, None),
            context,
        )
        .await;

        Ok(serde_json::to_value(&proposal)?)
    }
}
