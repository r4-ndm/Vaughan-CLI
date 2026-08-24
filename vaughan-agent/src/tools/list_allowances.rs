//! Read tool: list non-zero ERC-20 allowances vs known Ag/Dex/Bridge spenders.

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use vaughan_core::chains::evm::adapter::EvmAdapter;
use vaughan_core::chains::evm::networks::get_network_by_chain_id;
use vaughan_core::core::aggregator::OFFICIAL_AGG_ROUTERS;
use vaughan_core::core::bridge::OFFICIAL_ROUTERS;
use vaughan_core::core::wiz4rd::{deployment_for_chain, parse_addr, WZRD_SMOKE_943};
use vaughan_core::core::{dex_routers_labeled, wpls_for_chain};

#[derive(Default)]
pub struct ListAllowancesTool;

impl ListAllowancesTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ListAllowancesTool {
    fn name(&self) -> &str {
        "list_allowances"
    }

    fn description(&self) -> &str {
        "List non-zero ERC-20 allowances for the active account against known Dex/Ag/Bridge spenders. \
         Optional tokens[]; defaults include WPLS (+ WZRD on 943)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tokens": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional ERC-20 addresses to check (else known defaults)"
                },
                "account_address": {
                    "type": "string",
                    "description": "Owner to check (default: unlocked session address)"
                }
            }
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let owner = if let Some(s) = args.get("account_address").and_then(|v| v.as_str()) {
            Address::from_str(s)
                .map_err(|e| AgentError::InvalidToolCall(format!("Invalid account_address: {e}")))?
        } else {
            context.active_address.ok_or_else(|| {
                AgentError::InvalidToolCall(
                    "wallet_locked: unlock Vaughan or pass account_address".into(),
                )
            })?
        };

        let mut tokens: Vec<Address> = Vec::new();
        if let Some(arr) = args.get("tokens").and_then(|v| v.as_array()) {
            for t in arr {
                let s = t
                    .as_str()
                    .ok_or_else(|| AgentError::InvalidToolCall("tokens must be strings".into()))?;
                tokens.push(
                    Address::from_str(s)
                        .map_err(|e| AgentError::InvalidToolCall(format!("bad token: {e}")))?,
                );
            }
        }
        if tokens.is_empty() {
            if let Some(w) = wpls_for_chain(context.chain_id) {
                tokens.push(w);
            }
            if context.chain_id == 943 {
                if let Some(a) = parse_addr(WZRD_SMOKE_943) {
                    tokens.push(a);
                }
            }
            if let Some(dep) = deployment_for_chain(context.chain_id) {
                if let Some(w) = parse_addr(dep.wpls) {
                    if !tokens.contains(&w) {
                        tokens.push(w);
                    }
                }
            }
        }
        if tokens.is_empty() {
            return Err(AgentError::InvalidToolCall(
                "no tokens to check — pass tokens[]".into(),
            ));
        }

        let mut spenders: Vec<(Address, &'static str)> = dex_routers_labeled(context.chain_id);
        for s in OFFICIAL_AGG_ROUTERS {
            if let Ok(a) = Address::from_str(s) {
                spenders.push((a, "Ag"));
            }
        }
        if context.chain_id == 369 || context.chain_id == 943 {
            for s in OFFICIAL_ROUTERS {
                if let Ok(a) = Address::from_str(s) {
                    spenders.push((a, "Bridge"));
                }
            }
        }
        spenders.sort_by_key(|(a, _)| *a);
        spenders.dedup_by_key(|(a, _)| *a);

        let net = get_network_by_chain_id(context.chain_id);
        let net_name = net
            .as_ref()
            .map(|n| n.name.as_str())
            .unwrap_or("evm");
        let adapter = EvmAdapter::new(&context.rpc_url, context.chain_id, net_name, &[])
            .await
            .map_err(|e| AgentError::ProviderError(e.to_string()))?;

        let mut rows = Vec::new();
        for token in &tokens {
            for (spender, label) in &spenders {
                match adapter.get_erc20_allowance(*token, owner, *spender).await {
                    Ok(amount) if amount > U256::ZERO => {
                        rows.push(json!({
                            "token": format!("{token:#x}"),
                            "spender": format!("{spender:#x}"),
                            "spender_label": label,
                            "amount": amount.to_string(),
                        }));
                    }
                    _ => {}
                }
            }
        }

        Ok(json!({
            "owner": format!("{owner:#x}"),
            "chain_id": context.chain_id,
            "allowances": rows,
        }))
    }
}
