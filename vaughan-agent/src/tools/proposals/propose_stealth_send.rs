//! Propose a stealth native payment (pay destination + announce calldata).

use alloy::primitives::{Bytes, U256};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::{Tool, ToolContext};
use vaughan_core::security::stealth::{
    encode_announce_calldata, generate_stealth_address, native_announce_metadata,
    StealthMetaAddress, ERC5564_ANNOUNCER,
};

#[derive(Default)]
pub struct ProposeStealthSendTool;

impl ProposeStealthSendTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeStealthSendTool {
    fn name(&self) -> &str {
        "propose_stealth_send"
    }

    fn description(&self) -> &str {
        "Draft stealth native send: returns pay + announce TxProposals (approve both). \
         Recipient is an st:… meta-address URI. Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "recipient_uri": {
                    "type": "string",
                    "description": "st:<chain>:0x… stealth meta-address"
                },
                "amount": { "type": "string", "description": "Native wei to send" },
                "explanation": { "type": "string" }
            },
            "required": ["recipient_uri", "amount", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let _ = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("wallet_locked: unlock Vaughan or pass session".into())
        })?;
        let uri = args
            .get("recipient_uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing recipient_uri".into()))?;
        let amount = U256::from_str(
            args.get("amount")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing amount".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount: {e}")))?;
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?;

        let meta = StealthMetaAddress::parse(uri)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid stealth URI: {e}")))?;
        let announcement = generate_stealth_address(&meta, None)
            .map_err(|e| AgentError::InvalidToolCall(format!("stealth generate: {e}")))?;

        let pay = attach_estimated_fee(
            TxProposal::new(
                format!("stealth_pay_{}", rand_id()),
                ProposalType::NativeTransfer {
                    to: announcement.stealth_address,
                    amount_wei: amount,
                },
                announcement.stealth_address,
                amount,
                Bytes::new(),
                21_000,
                true,
                format!(
                    "{explanation} [stealth pay → {:#x}]",
                    announcement.stealth_address
                ),
            )
            .with_chain(context.chain_id, None),
            context,
        )
        .await;

        let metadata = native_announce_metadata(announcement.view_tag, amount);
        let announce_bytes = encode_announce_calldata(&announcement, &metadata);
        let announce = attach_estimated_fee(
            TxProposal::new(
                format!("stealth_announce_{}", rand_id()),
                ProposalType::ContractCall {
                    target: ERC5564_ANNOUNCER,
                    function_name: Some("announce".into()),
                },
                ERC5564_ANNOUNCER,
                U256::ZERO,
                announce_bytes,
                120_000,
                true,
                format!(
                    "{explanation} [stealth announce for {:#x}]",
                    announcement.stealth_address
                ),
            )
            .with_chain(context.chain_id, None),
            context,
        )
        .await;

        Ok(json!({
            "stealth_address": format!("{:#x}", announcement.stealth_address),
            "pay_proposal": pay,
            "announce_proposal": announce,
            "message": "Approve pay_proposal then announce_proposal (two cards / two auto-execs)"
        }))
    }
}
