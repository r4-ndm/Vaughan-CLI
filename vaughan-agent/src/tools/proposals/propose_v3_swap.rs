//! Proposal tool: wiz4rd V3 exact-in swap → TxProposal for TUI approval.

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::wiz4rd_common::{
    build_exact_in_calldata, load_pool, quote_pool, resolve_token, slippage_min_out,
};
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::is_allowed_dex_router;

#[derive(Default)]
pub struct ProposeV3SwapTool;

impl ProposeV3SwapTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV3SwapTool {
    fn name(&self) -> &str {
        "propose_v3_swap"
    }

    fn description(&self) -> &str {
        "Draft a wiz4rd V3 exact-in swap proposal for Vaughan TUI approval. \
         Never signs. Pulse testnet 943. Prefer quote_v3_swap first."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_in": {
                    "type": "string",
                    "description": "Input token (address, WPLS, WZRD, native/PLS)"
                },
                "token_out": {
                    "type": "string",
                    "description": "Output token (address, WPLS, WZRD, native/PLS)"
                },
                "amount_in": {
                    "type": "string",
                    "description": "Exact input amount in wei / raw units"
                },
                "fee": {
                    "type": "integer",
                    "default": 500
                },
                "slippage_bps": {
                    "type": "integer",
                    "description": "Slippage tolerance in basis points (100 = 1%)",
                    "default": 50
                },
                "explanation": {
                    "type": "string",
                    "description": "Untrusted agent rationale for the approval card"
                }
            },
            "required": ["token_in", "token_out", "amount_in", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let token_in_s = args
            .get("token_in")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_in".into()))?;
        let token_out_s = args
            .get("token_out")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_out".into()))?;
        let amount_in = U256::from_str(
            args.get("amount_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing amount_in".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount_in: {e}")))?;
        let fee = args.get("fee").and_then(|v| v.as_u64()).unwrap_or(500) as u32;
        let slippage_bps = args
            .get("slippage_bps")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as u32;
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?;

        let recipient = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall(
                "No active wallet — unlock Vaughan TUI or pass session account".into(),
            )
        })?;

        let (token_in, native_in) = resolve_token(token_in_s, context.chain_id)?;
        let (token_out, _) = resolve_token(token_out_s, context.chain_id)?;
        if token_in == token_out {
            return Err(AgentError::InvalidToolCall(
                "token_in and token_out must differ".into(),
            ));
        }

        let (cfg, pool) = load_pool(context, token_in, token_out, fee).await?;
        let quote = quote_pool(&pool, token_in, amount_in)?;
        let amount_out_min = slippage_min_out(quote.amount_out, slippage_bps);

        let (router, calldata, _) =
            build_exact_in_calldata(&cfg, &pool, token_in, amount_in, amount_out_min, recipient)?;

        if !is_allowed_dex_router(context.chain_id, router) {
            return Err(AgentError::InvalidToolCall(format!(
                "router {router:#x} not on DEX allowlist for chain {}",
                context.chain_id
            )));
        }

        let value_wei = if native_in { amount_in } else { U256::ZERO };
        let path = vec![token_in, token_out];

        // Pre-flight eth_call when not native (native needs balance); best-effort.
        let sim_success = if native_in {
            true
        } else {
            simulate_swap(context, router, recipient, &calldata, value_wei).await
        };

        let proposal = TxProposal::new(
            format!("v3_swap_{}", rand_id()),
            ProposalType::DexSwap {
                router,
                path,
                amount_in,
                min_amount_out: amount_out_min,
            },
            router,
            value_wei,
            calldata,
            350_000,
            sim_success,
            format!(
                "{explanation} [wiz4rd V3 fee {fee}: in {} → min out {}]",
                amount_in, amount_out_min
            ),
        )
        .with_chain(context.chain_id, Some("pulsechain-testnet-v4".into()));

        Ok(serde_json::to_value(&proposal)?)
    }
}

async fn simulate_swap(
    context: &ToolContext,
    router: Address,
    from: Address,
    calldata: &alloy::primitives::Bytes,
    value: U256,
) -> bool {
    use alloy::providers::Provider;
    use url::Url;

    let Ok(rpc_url) = Url::parse(&context.rpc_url) else {
        return false;
    };
    let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
        alloy::providers::RootProvider::new_http(rpc_url);
    let tx = alloy::rpc::types::eth::TransactionRequest::default()
        .from(from)
        .to(router)
        .input(calldata.clone().into())
        .value(value);
    provider.call(tx).await.is_ok()
}
