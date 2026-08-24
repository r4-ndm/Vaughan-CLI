//! Proposal tool: aggregator quote → TxProposal for human approval (Pulse DeFi trade).

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{assert_agg_exec_targets, quote_aggregator, AggQuoteRequest, AggVenue};

#[derive(Default)]
pub struct ProposeAggSwapTool;

impl ProposeAggSwapTool {
    pub fn new() -> Self {
        Self
    }
}

fn parse_venue(raw: &str) -> Result<AggVenue, AgentError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "squirrel" | "squirrelswap" => Ok(AggVenue::SquirrelSwap),
        "pulseswap" | "pulse" => Ok(AggVenue::PulseSwap),
        "piteas" => Ok(AggVenue::Piteas),
        "empx" | "empseal" => Ok(AggVenue::Empseal),
        other => Err(AgentError::InvalidToolCall(format!(
            "Unknown venue '{other}' — use squirrel, pulseswap, piteas, or empx"
        ))),
    }
}

fn parse_token_arg(raw: &str) -> Result<(Address, bool), AgentError> {
    let s = raw.trim();
    if s.eq_ignore_ascii_case("native")
        || s.eq_ignore_ascii_case("pls")
        || s.eq_ignore_ascii_case("eth")
    {
        return Ok((Address::ZERO, true));
    }
    let addr = Address::from_str(s)
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token address: {e}")))?;
    Ok((addr, false))
}

#[async_trait]
impl Tool for ProposeAggSwapTool {
    fn name(&self) -> &str {
        "propose_agg_swap"
    }

    fn description(&self) -> &str {
        "Quote a PulseChain aggregator swap and draft a TxProposal for TUI approval. \
         Never signs — human must approve in Vaughan. Prefer after quote_swap inspection."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "venue": {
                    "type": "string",
                    "description": "Aggregator: squirrel | pulseswap | piteas | empx",
                    "default": "squirrel"
                },
                "token_in": {
                    "type": "string",
                    "description": "Input token address, or 'native' / 'PLS'"
                },
                "token_out": {
                    "type": "string",
                    "description": "Output token address, or 'native' / 'PLS'"
                },
                "amount_in": {
                    "type": "string",
                    "description": "Input amount in raw base units (wei)"
                },
                "slippage_percent": {
                    "type": "number",
                    "default": 0.5
                },
                "explanation": {
                    "type": "string",
                    "description": "Untrusted agent rationale shown on the approval card"
                }
            },
            "required": ["token_in", "token_out", "amount_in", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let venue = parse_venue(
            args.get("venue")
                .and_then(|v| v.as_str())
                .unwrap_or("squirrel"),
        )?;
        let (token_in, native_in) = parse_token_arg(
            args.get("token_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token_in".into()))?,
        )?;
        let (token_out, native_out) = parse_token_arg(
            args.get("token_out")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token_out".into()))?,
        )?;
        let amount_in = U256::from_str(
            args.get("amount_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing amount_in".into()))?,
        )
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount_in: {e}")))?;
        let slippage = args
            .get("slippage_percent")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5);
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?;

        let req = AggQuoteRequest {
            token_in,
            token_out,
            token_in_is_native: native_in,
            token_out_is_native: native_out,
            amount_in,
            slippage_percent: slippage,
            account: context.active_address,
        };

        let quote = quote_aggregator(venue, &req, None, None)
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;

        assert_agg_exec_targets(quote.tx.to, quote.spender)
            .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;

        let gas_limit = quote.gas_estimate.unwrap_or(350_000).saturating_mul(12) / 10;
        let path = vec![
            if native_in { Address::ZERO } else { token_in },
            if native_out { Address::ZERO } else { token_out },
        ];

        let proposal = TxProposal::new(
            format!("agg_swap_{}", rand_id()),
            ProposalType::DexSwap {
                router: quote.tx.to,
                path,
                amount_in: quote.amount_in,
                min_amount_out: quote.amount_out,
            },
            quote.tx.to,
            quote.tx.value,
            quote.tx.data.clone(),
            gas_limit.max(100_000),
            true,
            format!(
                "{explanation} [{} → out {} via {}]",
                quote.amount_in,
                quote.amount_out,
                quote.venue.label()
            ),
        )
        .with_chain(context.chain_id, None);

        Ok(serde_json::to_value(&proposal)?)
    }
}
