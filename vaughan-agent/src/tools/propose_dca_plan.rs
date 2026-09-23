//! Draft a DCA plan for human approval (does not write disk until commit).

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::dca::{build_plan, build_proposal, DcaVenue};
use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

#[derive(Default)]
pub struct ProposeDcaPlanTool;

impl ProposeDcaPlanTool {
    pub fn new() -> Self {
        Self
    }
}

fn parse_venue(route: &str, venue: &str, protocol: &str) -> Result<DcaVenue, AgentError> {
    match route.trim().to_ascii_lowercase().as_str() {
        "" | "agg" | "aggregator" => Ok(DcaVenue::Agg {
            name: if venue.trim().is_empty() {
                "auto".into()
            } else {
                venue.trim().to_ascii_lowercase()
            },
        }),
        "dex" | "amm" => Ok(DcaVenue::Dex {
            name: if venue.trim().is_empty() {
                "pulsex".into()
            } else {
                venue.trim().to_ascii_lowercase()
            },
            protocol: if protocol.trim().is_empty() {
                "v2".into()
            } else {
                protocol.trim().to_ascii_lowercase()
            },
        }),
        other => Err(AgentError::InvalidToolCall(format!(
            "route must be 'agg' or 'dex', got '{other}'"
        ))),
    }
}

#[async_trait]
impl Tool for ProposeDcaPlanTool {
    fn name(&self) -> &str {
        "propose_dca_plan"
    }

    fn description(&self) -> &str {
        "Draft a recurring native→token buy (DCA) for human approval in the TUI. \
         Does not start buying until the human confirms. Choose route=agg (aggregator) \
         or route=dex (direct AMM) — meme tokens often need dex. Phase 1: time interval \
         only (interval_secs ≥ 900). Prefer dry_run=true on first create."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_out": {
                    "type": "string",
                    "description": "ERC-20 contract to buy (0x…)"
                },
                "slice_amount_wei": {
                    "type": "string",
                    "description": "Native PLS per slice in wei"
                },
                "interval_secs": {
                    "type": "integer",
                    "description": "Seconds between slices (min 900 = 15m; e.g. 14400 = 4h)"
                },
                "max_slices": {
                    "type": "integer",
                    "description": "Hard cap on number of buys"
                },
                "max_slippage_bps": {
                    "type": "integer",
                    "description": "Max slippage in basis points (default 100 = 1%)",
                    "default": 100
                },
                "route": {
                    "type": "string",
                    "description": "agg (aggregator) or dex (direct V2 AMM). Use dex when the token is missing from aggregators.",
                    "default": "agg"
                },
                "venue": {
                    "type": "string",
                    "description": "When route=agg: auto | squirrel | pulseswap | piteas | empx | 9mm. When route=dex: pulsex | 9inch | 9mm | sparkswap | … (catalog slug).",
                    "default": "auto"
                },
                "protocol": {
                    "type": "string",
                    "description": "DEX protocol when route=dex (v2 only for now)",
                    "default": "v2"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Paper-trade slices (default true)",
                    "default": true
                },
                "explanation": {
                    "type": "string",
                    "description": "Why this schedule — shown on the approval card"
                }
            },
            "required": [
                "token_out",
                "slice_amount_wei",
                "interval_secs",
                "max_slices",
                "explanation"
            ]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let token_out = args
            .get("token_out")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_out".into()))?;
        let slice = args
            .get("slice_amount_wei")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing slice_amount_wei".into()))?;
        let interval = args
            .get("interval_secs")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing interval_secs".into()))?;
        let max_slices = args
            .get("max_slices")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing max_slices".into()))?
            as u32;
        let slippage = args
            .get("max_slippage_bps")
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as u32;
        let route = args.get("route").and_then(|v| v.as_str()).unwrap_or("agg");
        let venue_name = args.get("venue").and_then(|v| v.as_str()).unwrap_or("auto");
        let protocol = args
            .get("protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("v2");
        let venue = parse_venue(route, venue_name, protocol)?;
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?;

        let account = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall(
                "wallet_locked: unlock Vaughan so propose_dca_plan can bind the plan to an address"
                    .into(),
            )
        })?;

        let plan = build_plan(
            token_out,
            slice,
            interval,
            venue,
            slippage,
            max_slices,
            dry_run,
            context.chain_id,
            account,
        )?;
        let proposal = build_proposal(plan, explanation);
        serde_json::to_value(&proposal)
            .map_err(|e| AgentError::ProviderError(format!("serialize dca proposal: {e}")))
    }
}
