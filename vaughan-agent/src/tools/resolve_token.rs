//! Resolve ERC-20 metadata; optionally persist into the profile wallet.json.

use alloy::primitives::Address;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::str::FromStr;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use vaughan_core::chains::evm::adapter::EvmAdapter;
use vaughan_core::chains::evm::networks::get_network_by_chain_id;
use vaughan_core::core::persistence::{CustomToken, StateManager};

#[derive(Default)]
pub struct ResolveTokenTool;

impl ResolveTokenTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResolveTokenTool {
    fn name(&self) -> &str {
        "resolve_token"
    }

    fn description(&self) -> &str {
        "Fetch ERC-20 symbol/name/decimals for a contract address (read-only)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token": { "type": "string", "description": "ERC-20 contract 0x…" }
            },
            "required": ["token"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let token = args
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token".into()))?;
        let _ = Address::from_str(token.trim())
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token: {e}")))?;
        let net = get_network_by_chain_id(context.chain_id);
        let name = net.as_ref().map(|n| n.name.as_str()).unwrap_or("evm");
        let adapter = EvmAdapter::new(&context.rpc_url, context.chain_id, name, &[])
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;
        let (symbol, token_name, decimals) = adapter
            .get_token_metadata(token.trim())
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;
        let addr = Address::from_str(token.trim()).unwrap();
        Ok(json!({
            "address": format!("{addr:#x}"),
            "symbol": symbol,
            "name": token_name,
            "decimals": decimals,
            "chain_id": context.chain_id,
        }))
    }
}

/// Import token into profile `wallet.json` custom_tokens (no vault unlock required).
pub struct ImportTokenTool {
    profile_dir: PathBuf,
}

impl ImportTokenTool {
    pub fn new(profile_dir: PathBuf) -> Self {
        Self { profile_dir }
    }
}

#[async_trait]
impl Tool for ImportTokenTool {
    fn name(&self) -> &str {
        "import_token"
    }

    fn description(&self) -> &str {
        "Resolve ERC-20 metadata and persist into the active Vaughan profile Assets list."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token": { "type": "string" }
            },
            "required": ["token"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let token = args
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token".into()))?;
        let addr = Address::from_str(token.trim())
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token: {e}")))?;
        let net = get_network_by_chain_id(context.chain_id);
        let name = net.as_ref().map(|n| n.name.as_str()).unwrap_or("evm");
        let adapter = EvmAdapter::new(&context.rpc_url, context.chain_id, name, &[])
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;
        let (symbol, token_name, decimals) = adapter
            .get_token_metadata(token.trim())
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;

        let wallet_path = self.profile_dir.join("wallet.json");
        // default profile may store wallet at parent of profiles — StateManager path varies.
        let sm = if wallet_path.exists() {
            StateManager::new(wallet_path)
        } else {
            // fall back: profile_dir might already be the data dir for default
            StateManager::new(self.profile_dir.join("wallet.json"))
        };
        let mut state = sm
            .load()
            .map_err(|e| AgentError::ProviderError(format!("load wallet.json: {e}")))?;
        let entry = CustomToken {
            chain_id: context.chain_id,
            address: format!("{addr:#x}"),
            symbol: symbol.clone(),
            name: token_name.clone(),
            decimals,
        };
        if !state
            .custom_tokens
            .iter()
            .any(|t| t.chain_id == entry.chain_id && t.address.eq_ignore_ascii_case(&entry.address))
        {
            state.custom_tokens.push(entry.clone());
            sm.save(&state)
                .map_err(|e| AgentError::ProviderError(format!("save wallet.json: {e}")))?;
        }
        Ok(json!({
            "imported": true,
            "address": entry.address,
            "symbol": symbol,
            "name": token_name,
            "decimals": decimals,
            "chain_id": context.chain_id,
        }))
    }
}
