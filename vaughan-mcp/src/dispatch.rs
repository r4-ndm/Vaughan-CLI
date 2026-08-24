//! Tool dispatch for MCP — maps tool names to `vaughan-agent` registry.

use alloy::primitives::Address;
use serde_json::{json, Value};
use vaughan_agent::paths::profile_dir;
use vaughan_agent::tools::{default_assist_registry, default_sensory_registry, ToolContext, ToolRegistry};
use vaughan_agent::AgentError;
use vaughan_core::chains::evm::adapter::EvmAdapter;
use vaughan_core::chains::evm::networks::get_network_by_id;
use vaughan_core::core::proposal::{
    guard_mainnet_write, ProposalQueue, TxProposal, McpSessionToken,
};
use vaughan_core::core::persistence::StateManager;

use crate::client::{try_get_session, try_proposal_status, try_propose_live};

/// MCP runtime context (no vault unlock — read tools use RPC + optional address).
#[derive(Debug, Clone)]
pub struct McpContext {
    pub profile: String,
    pub rpc_url: String,
    pub chain_id: u64,
    pub network_id: String,
    pub is_testnet: bool,
    pub active_address: Option<Address>,
    pub source: String,
}

pub struct McpDispatcher {
    sensory: ToolRegistry,
    assist: ToolRegistry,
    profile_dir: std::path::PathBuf,
}

impl McpDispatcher {
    pub fn new(profile: &str) -> Result<Self, String> {
        let wallet_path = StateManager::profile_path(profile)
            .map_err(|e| format!("profile path: {}", e.user_message()))?;
        let profile_dir = profile_dir(&wallet_path);
        Ok(Self {
            sensory: default_sensory_registry(),
            assist: default_assist_registry(),
            profile_dir,
        })
    }

    pub fn tool_definitions(&self) -> Vec<Value> {
        let mut tools = Vec::new();
        for def in self.sensory.definitions() {
            tools.push(json!({
                "name": def.name,
                "description": def.description,
                "inputSchema": def.parameters,
            }));
        }
        for def in self.assist.definitions() {
            if def.name.starts_with("propose_") {
                tools.push(json!({
                    "name": def.name,
                    "description": def.description,
                    "inputSchema": def.parameters,
                }));
            }
        }
        tools.push(json!({
            "name": "get_proposal_status",
            "description": "Get lifecycle status of a pending or completed proposal.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "proposal_id": { "type": "string" }
                },
                "required": ["proposal_id"]
            }
        }));
        tools.push(json!({
            "name": "list_pending_proposals",
            "description": "List all pending proposals in the file queue.",
            "inputSchema": { "type": "object", "properties": {} }
        }));
        tools
    }

    pub async fn call_tool(&self, name: &str, args: Value, ctx: &McpContext) -> Result<Value, String> {
        let ctx = self.refresh_context(ctx).await;
        match name {
            "get_proposal_status" => self.get_proposal_status(args, &ctx).await,
            "list_pending_proposals" => self.list_pending_proposals(&ctx),
            name if name.starts_with("propose_") => self.propose_tool(name, args, &ctx).await,
            name if self.sensory.definitions().iter().any(|d| d.name == name)
                || name == "get_balance" || name == "list_assets" || name == "get_network" || name == "get_address" =>
            {
                self.read_tool(name, args, &ctx).await
            }
            _ => Err(format!("unknown tool: {name}")),
        }
    }

    async fn read_tool(&self, name: &str, args: Value, ctx: &McpContext) -> Result<Value, String> {
        let tool_ctx = ToolContext {
            rpc_url: ctx.rpc_url.clone(),
            chain_id: ctx.chain_id,
            active_address: ctx.active_address,
        };

        match name {
            "get_network" => Ok(json!({
                "network_id": ctx.network_id,
                "chain_id": ctx.chain_id,
                "is_testnet": ctx.is_testnet,
                "rpc_url": ctx.rpc_url,
            })),
            "get_address" => {
                let addr = ctx.active_address.ok_or_else(|| {
                    "wallet_locked: unlock Vaughan TUI or pass account_address".to_string()
                })?;
                Ok(json!({ "address": format!("{addr:#x}") }))
            }
            "list_assets" => self.list_assets(ctx).await,
            _ => self
                .sensory
                .execute(name, args, &tool_ctx)
                .await
                .map_err(agent_err)
        }
    }

    /// Merge live TUI session data (address, network) when the wallet is unlocked.
    pub async fn refresh_context(&self, ctx: &McpContext) -> McpContext {
        let mut out = ctx.clone();
        let Ok(Some(token)) = McpSessionToken::read(&self.profile_dir) else {
            return out;
        };
        if token.is_empty() {
            return out;
        }
        let Ok(Some(session)) = try_get_session(&token).await else {
            return out;
        };
        out.active_address = Some(session.address);
        out.chain_id = session.chain_id;
        out.network_id = session.network_id.clone();
        if let Some(net) = get_network_by_id(&out.network_id) {
            out.rpc_url = net.rpc_url.clone();
            out.is_testnet = net.is_testnet;
        }
        out
    }

    async fn list_assets(&self, ctx: &McpContext) -> Result<Value, String> {
        let addr = ctx.active_address.ok_or_else(|| {
            "wallet_locked: unlock Vaughan TUI or pass account_address".to_string()
        })?;
        let net = get_network_by_id(&ctx.network_id)
            .ok_or_else(|| format!("unknown network: {}", ctx.network_id))?;
        let adapter = EvmAdapter::new(&ctx.rpc_url, ctx.chain_id, &net.name, &net.fallback_rpc_urls)
            .await
            .map_err(|e| e.to_string())?;
        let assets = adapter
            .get_assets(&format!("{addr:#x}"), &[])
            .await
            .map_err(|e| e.to_string())?;
        let rows: Vec<_> = assets
            .iter()
            .map(|bal| {
                json!({
                    "symbol": bal.token.symbol,
                    "formatted": bal.formatted,
                    "contract": bal.token.contract_address,
                })
            })
            .collect();
        Ok(json!({
            "address": format!("{addr:#x}"),
            "assets": rows,
        }))
    }

    async fn propose_tool(&self, name: &str, args: Value, ctx: &McpContext) -> Result<Value, String> {
        guard_mainnet_write(ctx.chain_id, ctx.is_testnet).map_err(|e| e.to_string())?;

        let tool_ctx = ToolContext {
            rpc_url: ctx.rpc_url.clone(),
            chain_id: ctx.chain_id,
            active_address: ctx.active_address,
        };

        let raw = self
            .assist
            .execute(name, args, &tool_ctx)
            .await
            .map_err(agent_err)?;
        let proposal: TxProposal =
            serde_json::from_value(raw).map_err(|e| format!("invalid proposal: {e}"))?;

        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .unwrap_or_default();

        if !session.is_empty() {
            match try_propose_live(&session, &ctx.source, &proposal).await {
                Ok(Some(data)) => return Ok(data),
                Ok(None) => {}
                Err(e) if e.contains("wallet is locked") || e.contains("tui_offline") => {}
                Err(e) => return Err(e),
            }
        }

        let secret = session.as_bytes();
        let queue = ProposalQueue::new(&self.profile_dir);
        let queued = queue
            .enqueue(proposal.clone(), &ctx.source, secret)
            .map_err(|e| e.to_string())?;

        Ok(json!({
            "proposal_id": queued.proposal.proposal_id,
            "status": "pending_user",
            "chain_id": queued.proposal.chain_id,
            "message": "Proposal queued — open and unlock Vaughan to approve",
        }))
    }

    async fn get_proposal_status(&self, args: Value, _ctx: &McpContext) -> Result<Value, String> {
        let proposal_id = args
            .get("proposal_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing proposal_id".to_string())?;

        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .unwrap_or_default();
        if !session.is_empty() {
            if let Ok(Some(data)) = try_proposal_status(&session, proposal_id).await {
                return Ok(data);
            }
        }

        let queue = ProposalQueue::new(&self.profile_dir);
        match queue.get_pending(proposal_id, session.as_bytes()) {
            Ok(_) => Ok(json!({
                "proposal_id": proposal_id,
                "status": "pending_user",
            })),
            Err(e) => Ok(json!({
                "proposal_id": proposal_id,
                "status": "unknown",
                "error": e.code(),
            })),
        }
    }

    fn list_pending_proposals(&self, _ctx: &McpContext) -> Result<Value, String> {
        let queue = ProposalQueue::new(&self.profile_dir);
        let pending = queue
            .list_pending()
            .map_err(|e| e.to_string())?;
        let ids: Vec<_> = pending
            .iter()
            .map(|q| {
                json!({
                    "proposal_id": q.proposal.proposal_id,
                    "source": q.source,
                    "chain_id": q.proposal.chain_id,
                })
            })
            .collect();
        Ok(json!({ "pending": ids }))
    }
}

fn agent_err(e: AgentError) -> String {
    match e {
        AgentError::InvalidToolCall(msg) => format!("invalid_tool_call: {msg}"),
        other => other.to_string(),
    }
}
