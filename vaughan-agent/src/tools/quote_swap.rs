//! Sensory tool: aggregator quote (Pulse DeFi skill pack — read-only).

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{quote_aggregator, AggQuoteRequest, AggVenue};

#[derive(Default)]
pub struct QuoteSwapTool;

impl QuoteSwapTool {
    pub fn new() -> Self {
        Self
    }
}

fn parse_venue(raw: &str) -> Result<AggVenue, AgentError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "squirrel" | "squirrelswap" => Ok(AggVenue::SquirrelSwap),
        "pulseswap" | "pulse" => Ok(AggVenue::PulseSwap),
        "piteas" => Ok(AggVenue::Piteas),
        other => Err(AgentError::InvalidToolCall(format!(
            "Unknown venue '{other}' — use squirrel, pulseswap, or piteas"
        ))),
    }
}

#[async_trait]
impl Tool for QuoteSwapTool {
    fn name(&self) -> &str {
        "quote_swap"
    }

    fn description(&self) -> &str {
        "Fetch a PulseChain aggregator quote (Squirrel / PulseSwap / Piteas). \
         Read-only — does not propose or sign. Use propose_agg_swap to turn a quote into an approval card."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "venue": {
                    "type": "string",
                    "description": "Aggregator: squirrel | pulseswap | piteas",
                    "default": "squirrel"
                },
                "token_in": {
                    "type": "string",
                    "description": "Input token address, or 'native' / 'PLS' for native coin"
                },
                "token_out": {
                    "type": "string",
                    "description": "Output token address, or 'native' / 'PLS' for native coin"
                },
                "amount_in": {
                    "type": "string",
                    "description": "Input amount in raw base units (wei)"
                },
                "slippage_percent": {
                    "type": "number",
                    "description": "Slippage percent (e.g. 0.5)",
                    "default": 0.5
                }
            },
            "required": ["token_in", "token_out", "amount_in"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let venue = args
            .get("venue")
            .and_then(|v| v.as_str())
            .unwrap_or("squirrel");
        let venue = parse_venue(venue)?;

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
        if native_in && native_out {
            return Err(AgentError::InvalidToolCall(
                "token_in and token_out cannot both be native".into(),
            ));
        }

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

        let mut req = AggQuoteRequest {
            token_in,
            token_out,
            token_in_is_native: native_in,
            token_out_is_native: native_out,
            amount_in,
            slippage_percent: slippage,
            account: context.active_address,
        };
        if let Some(account) = context.active_address {
            req.account = Some(account);
        }

        let quote = quote_aggregator(venue, &req, None, None)
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;

        Ok(json!({
            "venue": quote.venue.label(),
            "amount_in": quote.amount_in.to_string(),
            "amount_out": quote.amount_out.to_string(),
            "gas_estimate": quote.gas_estimate,
            "router": format!("{:#x}", quote.tx.to),
            "spender": format!("{:#x}", quote.spender),
            "value_wei": quote.tx.value.to_string(),
            "calldata": format!("0x{}", hex::encode(&quote.tx.data)),
            "chain_id": context.chain_id,
            "note": "Calldata is for inspection — use propose_agg_swap to queue for human approval",
        }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venue_aliases() {
        assert_eq!(parse_venue("squirrel").unwrap(), AggVenue::SquirrelSwap);
        assert_eq!(parse_venue("PulseSwap").unwrap(), AggVenue::PulseSwap);
        assert!(parse_venue("unknown").is_err());
    }
}
