//! DexScreener sensory tools (discovery + address-keyed identity).
//!
//! Thin wrappers around [`vaughan_core::core::DexScreenerClient`]. Search is
//! discovery-only; prefer address tools for settlement identity.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::DexScreenerClient;

fn optional_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

fn optional_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn client() -> Result<DexScreenerClient, AgentError> {
    DexScreenerClient::new().map_err(|e| AgentError::ProviderError(e.to_string()))
}

/// `dexscreener_search` — symbol/name discovery (PulseChain default).
#[derive(Default)]
pub struct DexscreenerSearchTool;

impl DexscreenerSearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DexscreenerSearchTool {
    fn name(&self) -> &str {
        "dexscreener_search"
    }

    fn description(&self) -> &str {
        "Search DexScreener pairs — discovery-only (defaults PulseChain). \
         May include ticker spoofs; read catalog_coverage and recommended_address_followups. \
         Never settle token identity from search alone — use dexscreener_token_pairs / \
         resolve_token with a verified 0x."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Symbol, name, or 0x address (symbol is discovery-only)"
                },
                "chain_id": {
                    "type": "integer",
                    "description": "Vaughan chain id (default 369 → pulsechain)"
                },
                "dex_chain": {
                    "type": "string",
                    "description": "DexScreener chain slug override (e.g. pulsechain)"
                },
                "pulsechain_only": {
                    "type": "boolean",
                    "description": "Keep only pairs on the resolved chain (default true)",
                    "default": true
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing query".into()))?;
        let chain_id = optional_u64(&args, "chain_id").or(Some(context.chain_id));
        let dex_chain = optional_str(&args, "dex_chain");
        let pulse_only = args
            .get("pulsechain_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        client()?
            .search(query, chain_id, dex_chain, pulse_only)
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))
    }
}

/// `dexscreener_token_pairs` — pools for a verified token address.
#[derive(Default)]
pub struct DexscreenerTokenPairsTool;

impl DexscreenerTokenPairsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DexscreenerTokenPairsTool {
    fn name(&self) -> &str {
        "dexscreener_token_pairs"
    }

    fn description(&self) -> &str {
        "List DexScreener pools for a token contract address (identity by address). \
         Origin labels attached when the address is in Vaughan’s PulseChain catalog."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token": {
                    "type": "string",
                    "description": "Token contract 0x…"
                },
                "chain_id": { "type": "integer" },
                "dex_chain": { "type": "string" }
            },
            "required": ["token"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let token = args
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token".into()))?;
        let chain_id = optional_u64(&args, "chain_id").or(Some(context.chain_id));
        client()?
            .token_pairs(token, chain_id, optional_str(&args, "dex_chain"))
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))
    }
}

/// `dexscreener_pair` — single LP / pair by address.
#[derive(Default)]
pub struct DexscreenerPairTool;

impl DexscreenerPairTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DexscreenerPairTool {
    fn name(&self) -> &str {
        "dexscreener_pair"
    }

    fn description(&self) -> &str {
        "Get one DexScreener pair by pair/LP address (identity by address)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pair": {
                    "type": "string",
                    "description": "Pair / LP contract 0x…"
                },
                "chain_id": { "type": "integer" },
                "dex_chain": { "type": "string" }
            },
            "required": ["pair"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let pair = args
            .get("pair")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing pair".into()))?;
        let chain_id = optional_u64(&args, "chain_id").or(Some(context.chain_id));
        client()?
            .pair(pair, chain_id, optional_str(&args, "dex_chain"))
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))
    }
}

/// `dexscreener_tokens` — batch token addresses → pairs (max 30).
#[derive(Default)]
pub struct DexscreenerTokensTool;

impl DexscreenerTokensTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DexscreenerTokensTool {
    fn name(&self) -> &str {
        "dexscreener_tokens"
    }

    fn description(&self) -> &str {
        "DexScreener pairs for one or more token addresses (array or comma-separated, max 30)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tokens": {
                    "description": "Token 0x addresses: JSON array or comma-separated string",
                    "oneOf": [
                        { "type": "array", "items": { "type": "string" } },
                        { "type": "string" }
                    ]
                },
                "chain_id": { "type": "integer" },
                "dex_chain": { "type": "string" }
            },
            "required": ["tokens"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let tokens = parse_token_list(&args)?;
        let chain_id = optional_u64(&args, "chain_id").or(Some(context.chain_id));
        client()?
            .tokens(&tokens, chain_id, optional_str(&args, "dex_chain"))
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))
    }
}

fn parse_token_list(args: &Value) -> Result<Vec<String>, AgentError> {
    let raw = args
        .get("tokens")
        .ok_or_else(|| AgentError::InvalidToolCall("Missing tokens".into()))?;
    if let Some(arr) = raw.as_array() {
        return Ok(arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect());
    }
    if let Some(s) = raw.as_str() {
        return Ok(s
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect());
    }
    Err(AgentError::InvalidToolCall(
        "tokens must be an array or comma-separated string".into(),
    ))
}
