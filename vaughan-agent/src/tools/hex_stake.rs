//! HEX stake sensory tools (`hex_global_state`, `hex_stakes_for_address`).
//!
//! Soft-fail JSON envelopes; never invents stake state. Writes live in
//! `propose_hex_stake_*`.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{fetch_hex_global_state, fetch_hex_stakes_for_address};

fn optional_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn optional_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
}

/// `hex_global_state` — currentDay + globals on pHEX (default).
#[derive(Default)]
pub struct HexGlobalStateTool;

impl HexGlobalStateTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for HexGlobalStateTool {
    fn name(&self) -> &str {
        "hex_global_state"
    }

    fn description(&self) -> &str {
        "On-chain HEX global stake state (currentDay, shareRate, lockedHeartsTotal, …). \
         Default contract=phex (PulseChain state-fork stakeable HEX). eHEX soft-fails \
         (bridged ERC-20 only). Hearts use 8 decimals. Not a price oracle. Read-only."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "contract": {
                    "type": "string",
                    "description": "phex | ehex | 0x address (default phex)",
                    "default": "phex"
                }
            }
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let which = optional_str(&args, "contract").unwrap_or("phex");
        let result = fetch_hex_global_state(&context.rpc_url, which).await;
        Ok(serde_json::to_value(result).map_err(|e| AgentError::ProviderError(e.to_string()))?)
    }
}

/// `hex_stakes_for_address` — stakeCount + stakeLists for a staker.
#[derive(Default)]
pub struct HexStakesForAddressTool;

impl HexStakesForAddressTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for HexStakesForAddressTool {
    fn name(&self) -> &str {
        "hex_stakes_for_address"
    }

    fn description(&self) -> &str {
        "List on-chain HEX stakes for a staker (stakeCount + stakeLists). Default \
         contract=phex. eHEX soft-fails. Hearts use 8 decimals. Advisory only — no \
         stake write. Prefer active wallet address when listing the user's stakes."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "staker": {
                    "type": "string",
                    "description": "Staker EOA/contract address (0x…)"
                },
                "contract": {
                    "type": "string",
                    "description": "phex | ehex | 0x address (default phex)",
                    "default": "phex"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max stakes to return (default 25, max 100)",
                    "default": 25
                }
            },
            "required": ["staker"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let staker = args
            .get("staker")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing staker".into()))?;
        let which = optional_str(&args, "contract").unwrap_or("phex");
        let limit = optional_usize(&args, "limit").unwrap_or(25);
        let result = fetch_hex_stakes_for_address(&context.rpc_url, staker, which, limit).await;
        Ok(serde_json::to_value(result).map_err(|e| AgentError::ProviderError(e.to_string()))?)
    }
}
