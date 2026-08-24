//! Sensory tool: quote snapshot for sentient “if price then act” loops.

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::{quote_aggregator, AggQuoteRequest, AggVenue};

#[derive(Default)]
pub struct WatchQuoteTool;

impl WatchQuoteTool {
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

fn parse_token(raw: &str) -> Result<(Address, bool), AgentError> {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("native")
        || t.eq_ignore_ascii_case("pls")
        || t.eq_ignore_ascii_case("tpls")
    {
        return Ok((Address::ZERO, true));
    }
    let addr = Address::from_str(t)
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token address: {e}")))?;
    Ok((addr, false))
}

#[async_trait]
impl Tool for WatchQuoteTool {
    fn name(&self) -> &str {
        "watch_quote"
    }

    fn description(&self) -> &str {
        "Snapshot an aggregator quote and flag threshold crossings (min_out_wei / max_out_wei). \
         Building block for sentient ‘if price then propose_agg_swap’ loops. Read-only."
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
                "token_in": { "type": "string" },
                "token_out": { "type": "string" },
                "amount_in": { "type": "string", "description": "Raw wei / base units" },
                "slippage_percent": { "type": "number", "default": 0.5 },
                "min_out_wei": {
                    "type": "string",
                    "description": "Alert when quoted amount_out is below this"
                },
                "max_out_wei": {
                    "type": "string",
                    "description": "Alert when quoted amount_out is above this"
                }
            },
            "required": ["token_in", "token_out", "amount_in"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let venue = parse_venue(
            args.get("venue")
                .and_then(|v| v.as_str())
                .unwrap_or("squirrel"),
        )?;
        let (token_in, native_in) = parse_token(
            args.get("token_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token_in".into()))?,
        )?;
        let (token_out, native_out) = parse_token(
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

        let req = AggQuoteRequest {
            token_in,
            token_out,
            token_in_is_native: native_in,
            token_out_is_native: native_out,
            amount_in,
            slippage_percent: slippage,
            account: context.active_address,
        };
        let quote = quote_aggregator(venue, &req, context.chain_id, None, None)
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;

        let out = quote.amount_out;
        let mut below_min = false;
        let mut above_max = false;
        if let Some(m) = args.get("min_out_wei").and_then(|v| v.as_str()) {
            if let Ok(min) = U256::from_str(m) {
                below_min = out < min;
            }
        }
        if let Some(m) = args.get("max_out_wei").and_then(|v| v.as_str()) {
            if let Ok(max) = U256::from_str(m) {
                above_max = out > max;
            }
        }
        let alert = below_min || above_max;
        Ok(json!({
            "venue": quote.venue.label(),
            "amount_in": quote.amount_in.to_string(),
            "amount_out": out.to_string(),
            "router": format!("{:#x}", quote.tx.to),
            "below_min": below_min,
            "above_max": above_max,
            "alert": alert,
            "suggested_action": if alert {
                "propose_agg_swap"
            } else {
                "wait"
            },
        }))
    }
}
