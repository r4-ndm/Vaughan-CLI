//! Sensory tool: Get native balance or ERC-20 token balance.

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use url::Url;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

sol! {
    interface IERC20Balance {
        function balanceOf(address account) external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
    }
}

#[derive(Default)]
pub struct GetBalanceTool;

impl GetBalanceTool {
    pub fn new() -> Self {
        Self
    }
}

/// Resolve the account to query: prefer explicit arg, else session `active_address`.
/// Never silently query the zero address when a connected wallet exists.
fn resolve_account(args: &Value, context: &ToolContext) -> Result<(Address, bool), AgentError> {
    let raw = args
        .get("account_address")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let parsed =
        match raw {
            Some(s) => Some(Address::from_str(s).map_err(|e| {
                AgentError::InvalidToolCall(format!("Invalid account address: {e}"))
            })?),
            None => None,
        };

    match parsed {
        Some(addr) if !addr.is_zero() => Ok((addr, false)),
        Some(_) | None => {
            let Some(active) = context.active_address else {
                return Err(AgentError::InvalidToolCall(
                    "No account_address given and no connected wallet in session — \
                     unlock the vault or pass account_address"
                        .into(),
                ));
            };
            if active.is_zero() {
                return Err(AgentError::InvalidToolCall(
                    "Connected wallet address is zero — cannot query balance".into(),
                ));
            }
            // LLM passed 0x0 or omitted; substitute the real session account.
            Ok((active, parsed.is_some()))
        }
    }
}

#[async_trait]
impl Tool for GetBalanceTool {
    fn name(&self) -> &str {
        "get_balance"
    }

    fn description(&self) -> &str {
        "Query the native coin balance or ERC-20 token balance. \
         Omit account_address (or pass the SESSION CONTEXT wallet) to use the connected account. \
         Do not pass the zero address."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "account_address": {
                    "type": "string",
                    "description": "Optional. Defaults to the connected wallet from SESSION CONTEXT."
                },
                "token_address": {
                    "type": "string",
                    "description": "Optional ERC-20 token contract address (omit for native coin balance). Alias: token"
                },
                "token": {
                    "type": "string",
                    "description": "Alias for token_address when querying ERC-20 balance"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let (account, substituted_zero) = resolve_account(&args, context)?;

        let rpc_url = Url::parse(&context.rpc_url)
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;

        let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
            alloy::providers::RootProvider::new_http(rpc_url);

        if let Some(token_str) = args
            .get("token_address")
            .or_else(|| args.get("token"))
            .and_then(|v| v.as_str())
        {
            let token_addr = Address::from_str(token_str)
                .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token address: {e}")))?;

            let bal_call = IERC20Balance::balanceOfCall { account };
            let tx = alloy::rpc::types::eth::TransactionRequest::default()
                .to(token_addr)
                .input(bal_call.abi_encode().into());

            let res = provider.call(tx).await.map_err(|e| {
                AgentError::ProviderError(format!("Failed to query ERC-20 balance: {e}"))
            })?;

            let balance = if res.len() >= 32 {
                U256::from_be_slice(&res[..32]).to_string()
            } else {
                "0".to_string()
            };

            let mut out = json!({
                "account": format!("{account:#x}"),
                "token": format!("{token_addr:#x}"),
                "balance_raw": balance,
            });
            if substituted_zero {
                out.as_object_mut().unwrap().insert(
                    "note".into(),
                    json!("substituted zero address with connected session wallet"),
                );
            }
            Ok(out)
        } else {
            let native_balance = provider.get_balance(account).await.map_err(|e| {
                AgentError::ProviderError(format!("Failed to query native balance: {e}"))
            })?;

            let mut out = json!({
                "account": format!("{account:#x}"),
                "balance_wei": native_balance.to_string(),
            });
            if substituted_zero {
                out.as_object_mut().unwrap().insert(
                    "note".into(),
                    json!("substituted zero address with connected session wallet"),
                );
            }
            Ok(out)
        }
    }
}
