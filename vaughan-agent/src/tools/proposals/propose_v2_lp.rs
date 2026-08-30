//! 9inch V2 add / remove liquidity proposals (Pulse mainnet 369).

use alloy::primitives::{Address, Bytes, U256};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::transaction::parse_native_amount;
use vaughan_core::core::{
    build_v2_add_liquidity_evm, build_v2_remove_liquidity_evm, lp_v2_venue, venue_swap_router,
    wpls_for_chain, DexProtocol, DexVenue, DEFAULT_DEX_SLIPPAGE_BPS,
};

fn require_explanation(args: &Value) -> Result<&str, AgentError> {
    args.get("explanation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))
}

fn parse_addr(s: &str, label: &str) -> Result<Address, AgentError> {
    Address::from_str(s.trim()).map_err(|e| AgentError::InvalidToolCall(format!("{label}: {e}")))
}

fn mainnet_nine_inch() -> Result<DexVenue, AgentError> {
    lp_v2_venue(369)
        .filter(|v| *v == DexVenue::NineInch)
        .ok_or_else(|| AgentError::InvalidToolCall("9inch V2 LP is mainnet (369) only".into()))
}

#[derive(Default)]
pub struct ProposeV2AddTool;

impl ProposeV2AddTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV2AddTool {
    fn name(&self) -> &str {
        "propose_v2_add"
    }

    fn description(&self) -> &str {
        "Draft 9inch V2 add-liquidity on Pulse mainnet (369). Requires prior ERC-20 approve to router."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token0": { "type": "string" },
                "token1": { "type": "string" },
                "amount0": { "type": "string", "description": "Human amount for token0" },
                "amount1": { "type": "string", "description": "Human amount for token1" },
                "decimals0": { "type": "integer", "default": 18 },
                "decimals1": { "type": "integer", "default": 18 },
                "explanation": { "type": "string" }
            },
            "required": ["token0", "token1", "amount0", "amount1", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        if context.chain_id != 369 {
            return Err(AgentError::InvalidToolCall(
                "propose_v2_add is 9inch mainnet only".into(),
            ));
        }
        let venue = mainnet_nine_inch()?;
        let token0 = parse_addr(
            args.get("token0")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token0".into()))?,
            "token0",
        )?;
        let token1 = parse_addr(
            args.get("token1")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token1".into()))?,
            "token1",
        )?;
        let amount0 = args
            .get("amount0")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing amount0".into()))?;
        let amount1 = args
            .get("amount1")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing amount1".into()))?;
        let dec0 = args.get("decimals0").and_then(|v| v.as_u64()).unwrap_or(18) as u8;
        let dec1 = args.get("decimals1").and_then(|v| v.as_u64()).unwrap_or(18) as u8;
        let explanation = require_explanation(&args)?;
        let from = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall("wallet_locked: unlock Vaughan TUI".into())
        })?;
        let wpls = wpls_for_chain(369);
        let native = wpls.filter(|w| token0 == *w || token1 == *w);
        let evm = build_v2_add_liquidity_evm(
            &format!("{from:#x}"),
            venue,
            369,
            token0,
            token1,
            amount0,
            amount1,
            dec0,
            dec1,
            DEFAULT_DEX_SLIPPAGE_BPS,
            native,
        )
        .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        let router = venue_swap_router(venue, DexProtocol::V2, 369).unwrap();
        let calldata = hex::decode(evm.data.as_deref().unwrap_or("0x").trim_start_matches("0x"))
            .map_err(|e| AgentError::InvalidToolCall(format!("calldata: {e}")))?;
        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("v2add_{}", rand_id()),
                ProposalType::ContractCall {
                    target: router,
                    function_name: Some("addLiquidity".into()),
                },
                router,
                U256::from_str(&evm.value).unwrap_or_default(),
                Bytes::from(calldata),
                350_000,
                true,
                format!("{explanation} — 9inch V2 add liquidity"),
            )
            .with_chain(369, Some("pulsechain".into())),
            context,
        )
        .await;
        Ok(serde_json::to_value(&proposal)?)
    }
}

#[derive(Default)]
pub struct ProposeV2RemoveTool;

impl ProposeV2RemoveTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV2RemoveTool {
    fn name(&self) -> &str {
        "propose_v2_remove"
    }

    fn description(&self) -> &str {
        "Draft 9inch V2 remove-liquidity on Pulse mainnet (369). Pass full LP balance or a portion."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token0": { "type": "string" },
                "token1": { "type": "string" },
                "liquidity": { "type": "string", "description": "LP token amount (raw base units)" },
                "explanation": { "type": "string" }
            },
            "required": ["token0", "token1", "liquidity", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        if context.chain_id != 369 {
            return Err(AgentError::InvalidToolCall(
                "propose_v2_remove is 9inch mainnet only".into(),
            ));
        }
        let venue = mainnet_nine_inch()?;
        let token0 = parse_addr(
            args.get("token0")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token0".into()))?,
            "token0",
        )?;
        let token1 = parse_addr(
            args.get("token1")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token1".into()))?,
            "token1",
        )?;
        let liq = args
            .get("liquidity")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing liquidity".into()))?;
        let raw = parse_native_amount(liq.trim(), 18)
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        let liquidity = U256::from_str(&raw)
            .map_err(|e| AgentError::InvalidToolCall(format!("liquidity: {e}")))?;
        let explanation = require_explanation(&args)?;
        let from = context
            .active_address
            .ok_or_else(|| AgentError::InvalidToolCall("wallet_locked".into()))?;
        let wpls = wpls_for_chain(369);
        let native = wpls.filter(|w| token0 == *w || token1 == *w);
        let evm = build_v2_remove_liquidity_evm(
            &format!("{from:#x}"),
            venue,
            369,
            token0,
            token1,
            liquidity,
            DEFAULT_DEX_SLIPPAGE_BPS,
            native,
        )
        .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        let router = venue_swap_router(venue, DexProtocol::V2, 369).unwrap();
        let calldata = hex::decode(evm.data.as_deref().unwrap_or("0x").trim_start_matches("0x"))
            .map_err(|e| AgentError::InvalidToolCall(format!("calldata: {e}")))?;
        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("v2rm_{}", rand_id()),
                ProposalType::ContractCall {
                    target: router,
                    function_name: Some("removeLiquidity".into()),
                },
                router,
                U256::ZERO,
                Bytes::from(calldata),
                300_000,
                true,
                format!("{explanation} — 9inch V2 remove liquidity"),
            )
            .with_chain(369, Some("pulsechain".into())),
            context,
        )
        .await;
        Ok(serde_json::to_value(&proposal)?)
    }
}
