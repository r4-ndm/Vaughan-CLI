//! Propose HEX `stakeStart` / `stakeEnd` on pHEX (PulseChain).

use alloy::primitives::U256;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{
    encode_stake_end, encode_stake_start, resolve_hex_contract, MAX_STAKE_DAYS, MIN_STAKE_DAYS,
};

fn require_explanation(args: &Value) -> Result<String, AgentError> {
    args.get("explanation")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))
}

fn require_phex_target(args: &Value) -> Result<alloy::primitives::Address, AgentError> {
    use vaughan_core::core::{phex_address, HexContractKind};

    let which = args
        .get("contract")
        .and_then(|v| v.as_str())
        .unwrap_or("phex");
    let resolved =
        resolve_hex_contract(which).map_err(AgentError::InvalidToolCall)?;
    // Writes are pHEX-only — refuse eHEX and arbitrary custom addresses.
    if resolved.kind != HexContractKind::Phex || resolved.address != phex_address() {
        return Err(AgentError::InvalidToolCall(
            "HEX stake writes must target catalogued pHEX (contract=phex). \
             eHEX and custom addresses are refused — use hex_stakes_for_address for reads."
                .into(),
        ));
    }
    Ok(resolved.address)
}

/// `propose_hex_stake_start` — draft stakeStart(hearts, days).
#[derive(Default)]
pub struct ProposeHexStakeStartTool;

impl ProposeHexStakeStartTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeHexStakeStartTool {
    fn name(&self) -> &str {
        "propose_hex_stake_start"
    }

    fn description(&self) -> &str {
        "Draft a HEX stakeStart proposal on pHEX (hearts + staked days). Hearts use 8 \
         decimals (raw integer string). Days 1–5555. Never signs — user approves in TUI. \
         Writes are pHEX-only (eHEX and custom 0x refused)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "hearts": {
                    "type": "string",
                    "description": "Staked amount in Hearts (8 decimals, raw integer)"
                },
                "days": {
                    "type": "integer",
                    "description": format!("Stake length in HEX days ({MIN_STAKE_DAYS}–{MAX_STAKE_DAYS})")
                },
                "contract": {
                    "type": "string",
                    "description": "Must be phex / hex / ph (default). eHEX and custom 0x refused for writes.",
                    "default": "phex"
                },
                "explanation": { "type": "string" }
            },
            "required": ["hearts", "days", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let _ = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("wallet_locked: unlock Vaughan or pass session".into())
        })?;
        if context.chain_id != 369 {
            return Err(AgentError::InvalidToolCall(
                "HEX stakes are on PulseChain mainnet (chain_id 369)".into(),
            ));
        }
        let hearts = U256::from_str(
            args.get("hearts")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing hearts".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid hearts: {e}")))?;
        let days = args
            .get("days")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing days".into()))?;
        let explanation = require_explanation(&args)?;
        let target = require_phex_target(&args)?;
        let calldata = encode_stake_start(hearts, days)
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("hex_stake_start_{}", rand_id()),
                ProposalType::ContractCall {
                    target,
                    function_name: Some("stakeStart".into()),
                },
                target,
                U256::ZERO,
                calldata,
                350_000,
                true,
                explanation,
            )
            .with_chain(context.chain_id, Some("pulsechain".into())),
            context,
        )
        .await;
        Ok(serde_json::to_value(&proposal)?)
    }
}

/// `propose_hex_stake_end` — draft stakeEnd(index, stakeId).
#[derive(Default)]
pub struct ProposeHexStakeEndTool;

impl ProposeHexStakeEndTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeHexStakeEndTool {
    fn name(&self) -> &str {
        "propose_hex_stake_end"
    }

    fn description(&self) -> &str {
        "Draft a HEX stakeEnd proposal on pHEX (stake index + stakeId from \
         hex_stakes_for_address). Early end incurs a penalty. Writes are pHEX-only. \
         Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "stake_index": {
                    "type": "integer",
                    "description": "Index into stakeLists for the staker"
                },
                "stake_id": {
                    "type": "integer",
                    "description": "uint40 stakeId from stakeLists"
                },
                "contract": {
                    "type": "string",
                    "description": "Must be phex / hex / ph (default). eHEX and custom 0x refused for writes.",
                    "default": "phex"
                },
                "explanation": { "type": "string" }
            },
            "required": ["stake_index", "stake_id", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let _ = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("wallet_locked: unlock Vaughan or pass session".into())
        })?;
        if context.chain_id != 369 {
            return Err(AgentError::InvalidToolCall(
                "HEX stakes are on PulseChain mainnet (chain_id 369)".into(),
            ));
        }
        let stake_index = args
            .get("stake_index")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing stake_index".into()))?;
        let stake_id = args
            .get("stake_id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing stake_id".into()))?;
        let explanation = require_explanation(&args)?;
        let target = require_phex_target(&args)?;
        let calldata = encode_stake_end(stake_index, stake_id)
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("hex_stake_end_{}", rand_id()),
                ProposalType::ContractCall {
                    target,
                    function_name: Some("stakeEnd".into()),
                },
                target,
                U256::ZERO,
                calldata,
                350_000,
                true,
                explanation,
            )
            .with_chain(context.chain_id, Some("pulsechain".into())),
            context,
        )
        .await;
        Ok(serde_json::to_value(&proposal)?)
    }
}

#[cfg(test)]
mod tests {
    use super::require_phex_target;
    use serde_json::json;
    use vaughan_core::core::phex_address;

    #[test]
    fn propose_target_accepts_phex_aliases_only() {
        let addr = require_phex_target(&json!({})).unwrap();
        assert_eq!(addr, phex_address());
        assert_eq!(
            require_phex_target(&json!({ "contract": "HEX" })).unwrap(),
            phex_address()
        );
        assert!(require_phex_target(&json!({ "contract": "ehex" })).is_err());
        assert!(require_phex_target(&json!({
            "contract": "0x1111111111111111111111111111111111111111"
        }))
        .is_err());
        assert_eq!(
            require_phex_target(&json!({
                "contract": format!("{:#x}", phex_address())
            }))
            .unwrap(),
            phex_address()
        );
    }
}
