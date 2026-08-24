//! LibertySwap bridge quote + propose tools.

use alloy::primitives::{Address, Bytes, U256};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{BridgeAsset, BridgeQuote, BridgeQuoteRequest, LibertySwapClient};

fn parse_asset(s: &str) -> Result<BridgeAsset, AgentError> {
    let t = s.trim();
    if t.starts_with("0x") || t.starts_with("0X") {
        let a = Address::from_str(t)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token address: {e}")))?;
        Ok(BridgeAsset::Address(a))
    } else {
        // Leak static for API — common symbols only.
        let sym: &'static str = match t.to_ascii_uppercase().as_str() {
            "USDC" => "USDC",
            "USDT" => "USDT",
            "ETH" => "ETH",
            "WETH" => "WETH",
            "PLS" => "PLS",
            "WPLS" => "WPLS",
            other => {
                return Err(AgentError::InvalidToolCall(format!(
                    "unsupported bridge symbol `{other}` — use 0x address or USDC/ETH/…"
                )))
            }
        };
        Ok(BridgeAsset::Symbol(sym))
    }
}

fn quote_to_json(q: &BridgeQuote) -> Value {
    json!({
        "router": format!("{:#x}", q.to),
        "src_token": {
            "address": format!("{:#x}", q.src_token.address),
            "symbol": q.src_token.symbol,
            "decimals": q.src_token.decimals,
            "chain_id": q.src_token.chain_id,
        },
        "dest_token": {
            "address": format!("{:#x}", q.dest_token.address),
            "symbol": q.dest_token.symbol,
            "decimals": q.dest_token.decimals,
            "chain_id": q.dest_token.chain_id,
        },
        "src_amount": q.src_amount.to_string(),
        "dest_amount": q.dest_amount.to_string(),
        "fee_pct": q.fee.percentage,
        "fee_amount": q.fee.amount.to_string(),
        "approval": q.approval.as_ref().map(|a| json!({
            "token": format!("{:#x}", a.token),
            "spender": format!("{:#x}", a.spender),
            "amount": a.amount.to_string(),
        })),
        "tx": {
            "to": format!("{:#x}", q.tx.to),
            "value": q.tx.value.to_string(),
            "data": format!("0x{}", hex::encode(&q.tx.data)),
        }
    })
}

async fn fetch_quote(args: &Value, context: &ToolContext) -> Result<BridgeQuote, AgentError> {
    let recipient = if let Some(s) = args.get("recipient").and_then(|v| v.as_str()) {
        Address::from_str(s)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid recipient: {e}")))?
    } else {
        context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("wallet_locked: pass recipient or unlock session".into())
        })?
    };
    let src_token = parse_asset(
        args.get("src_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing src_token".into()))?,
    )?;
    let dst_token = parse_asset(
        args.get("dst_token")
            .and_then(|v| v.as_str())
            .unwrap_or("USDC"),
    )?;
    let amount = U256::from_str(
        args.get("amount")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing amount".into()))?,
    )
    .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount: {e}")))?;
    let src_chain = args
        .get("src_chain")
        .and_then(|v| v.as_u64())
        .unwrap_or(context.chain_id);
    let dst_chain = args
        .get("dst_chain")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| AgentError::InvalidToolCall("Missing dst_chain".into()))?;

    let client =
        LibertySwapClient::public().map_err(|e| AgentError::ProviderError(e.to_string()))?;
    let req = BridgeQuoteRequest {
        src_token,
        dst_token,
        amount,
        src_chain,
        dst_chain,
        recipient,
    };
    client
        .quote(&req)
        .await
        .map_err(|e| AgentError::ProviderError(e.to_string()))
}

#[derive(Default)]
pub struct QuoteBridgeTool;

impl QuoteBridgeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for QuoteBridgeTool {
    fn name(&self) -> &str {
        "quote_bridge"
    }

    fn description(&self) -> &str {
        "LibertySwap cross-chain quote (source broadcast only). Read-only; no keys."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "src_token": { "type": "string", "description": "USDC or 0x…" },
                "dst_token": { "type": "string", "default": "USDC" },
                "amount": { "type": "string", "description": "Amount in raw units" },
                "src_chain": { "type": "integer" },
                "dst_chain": { "type": "integer" },
                "recipient": { "type": "string" }
            },
            "required": ["src_token", "amount", "dst_chain"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let q = fetch_quote(&args, context).await?;
        Ok(quote_to_json(&q))
    }
}

#[derive(Default)]
pub struct ProposeBridgeTool;

impl ProposeBridgeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeBridgeTool {
    fn name(&self) -> &str {
        "propose_bridge"
    }

    fn description(&self) -> &str {
        "Draft LibertySwap source-chain bridge tx for Vaughan approval. \
         If quote needs ERC-20 approve, call propose_approve first. Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "src_token": { "type": "string" },
                "dst_token": { "type": "string", "default": "USDC" },
                "amount": { "type": "string" },
                "src_chain": { "type": "integer" },
                "dst_chain": { "type": "integer" },
                "recipient": { "type": "string" },
                "explanation": { "type": "string" }
            },
            "required": ["src_token", "amount", "dst_chain", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let _ = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("wallet_locked: unlock Vaughan or pass session".into())
        })?;
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?;
        let q = fetch_quote(&args, context).await?;
        let calldata = if q.tx.data.is_empty() {
            Bytes::new()
        } else {
            q.tx.data.clone()
        };
        let proposal = TxProposal::new(
            format!("bridge_{}", rand_id()),
            ProposalType::ContractCall {
                target: q.tx.to,
                function_name: Some("libertyBridge".into()),
            },
            q.tx.to,
            q.tx.value,
            calldata,
            500_000,
            true,
            format!(
                "{explanation} [LibertySwap {}→{} src={} dest≈{}]",
                q.src_token.chain_id, q.dest_token.chain_id, q.src_amount, q.dest_amount
            ),
        )
        .with_chain(q.src_token.chain_id, None);
        let mut out = serde_json::to_value(&proposal)?;
        if let Some(obj) = out.as_object_mut() {
            obj.insert("bridge_quote".into(), quote_to_json(&q));
        }
        Ok(out)
    }
}
