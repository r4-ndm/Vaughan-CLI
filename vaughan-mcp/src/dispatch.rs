//! Tool dispatch for MCP — maps tool names to `vaughan-agent` registry.

use alloy::primitives::Address;
use serde_json::{json, Value};
use vaughan_agent::paths::profile_dir;
use vaughan_agent::tools::{
    default_assist_registry_for, default_sensory_registry, ToolContext, ToolRegistry,
};
use vaughan_agent::AgentError;
use vaughan_core::chains::evm::adapter::EvmAdapter;
use vaughan_core::chains::evm::networks::get_network_by_id;
use vaughan_core::core::persistence::StateManager;
use vaughan_core::core::proposal::{
    guard_mainnet_write, proposal_status_json, McpSessionToken, ProposalQueue, TxProposal,
};

use crate::client::{
    ping, try_get_session, try_proposal_status, try_propose_live, try_stealth_scan,
    try_stealth_sweep, try_stealth_uri,
};

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
            assist: default_assist_registry_for(Some(&profile_dir)),
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
            if def.name.starts_with("propose_") || def.name == "import_token" {
                tools.push(json!({
                    "name": def.name,
                    "description": def.description,
                    "inputSchema": def.parameters,
                }));
            }
        }
        tools.push(json!({
            "name": "get_address",
            "description": "Active wallet address when Vaughan TUI is unlocked (session bridge).",
            "inputSchema": { "type": "object", "properties": {} }
        }));
        tools.push(json!({
            "name": "get_network",
            "description": "Active network id, chain id, and RPC for the MCP session.",
            "inputSchema": { "type": "object", "properties": {} }
        }));
        tools.push(json!({
            "name": "list_assets",
            "description": "Native + known ERC-20 balances for the unlocked active account.",
            "inputSchema": { "type": "object", "properties": {} }
        }));
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
        tools.push(json!({
            "name": "get_control_plane_status",
            "description": "Whether Vaughan TUI or `vaughan serve` is reachable on loopback, \
                 and whether the wallet session is unlocked. Sentients should poll this before writes.",
            "inputSchema": { "type": "object", "properties": {} }
        }));
        tools.push(json!({
            "name": "get_stealth_uri",
            "description": "This vault's ERC-5564 stealth meta-address URI (st:…). Requires unlocked TUI or vaughan serve.",
            "inputSchema": { "type": "object", "properties": {} }
        }));
        tools.push(json!({
            "name": "scan_stealth_notes",
            "description": "Scan for unswept stealth notes owned by this vault. Requires unlocked TUI or vaughan serve.",
            "inputSchema": { "type": "object", "properties": {} }
        }));
        tools.push(json!({
            "name": "sweep_stealth_note",
            "description": "Sweep one stealth note to the active account (approval card on adviser; auto on sentient).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "stealth_address": { "type": "string" }
                },
                "required": ["stealth_address"]
            }
        }));
        tools
    }

    pub async fn call_tool(
        &self,
        name: &str,
        args: Value,
        ctx: &McpContext,
    ) -> Result<Value, String> {
        let ctx = self.refresh_context(ctx).await;
        match name {
            "get_proposal_status" => self.get_proposal_status(args, &ctx).await,
            "list_pending_proposals" => self.list_pending_proposals(&ctx),
            "get_control_plane_status" => self.control_plane_status(&ctx).await,
            "import_token" => self.assist_side_effect("import_token", args, &ctx).await,
            "get_stealth_uri" => self.stealth_uri(&ctx).await,
            "scan_stealth_notes" => self.stealth_scan(&ctx).await,
            "sweep_stealth_note" => self.stealth_sweep(args, &ctx).await,
            name if name.starts_with("propose_") => self.propose_tool(name, args, &ctx).await,
            name if self.sensory.definitions().iter().any(|d| d.name == name)
                || name == "get_balance"
                || name == "list_assets"
                || name == "get_network"
                || name == "get_address" =>
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
            "get_network" => {
                let mut out = json!({
                    "network_id": ctx.network_id,
                    "chain_id": ctx.chain_id,
                    "is_testnet": ctx.is_testnet,
                    "rpc_url": ctx.rpc_url,
                });
                if let Some(wiz) = vaughan_core::core::deployment_for_chain(ctx.chain_id) {
                    out["wiz4rd"] = json!({
                        "factory": wiz.factory,
                        "pool_deployer": wiz.pool_deployer,
                        "swap_router": wiz.swap_router,
                        "position_manager": wiz.position_manager,
                        "quoter_v2": wiz.quoter_v2,
                        "tick_lens": wiz.tick_lens,
                        "wpls": wiz.wpls,
                        "smoke_token_wzrd": vaughan_core::core::wiz4rd::WZRD_SMOKE_943,
                        "smoke_pool_wzrd_wpls_500": vaughan_core::core::wiz4rd::SMOKE_POOL_WZRD_WPLS_500_943,
                        "fee_tiers": vaughan_core::core::WIZ4RD_FEE_TIERS,
                    });
                }
                Ok(out)
            }
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
                .map_err(agent_err),
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
        let adapter = EvmAdapter::new(
            &ctx.rpc_url,
            ctx.chain_id,
            &net.name,
            &net.fallback_rpc_urls,
        )
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

    async fn propose_tool(
        &self,
        name: &str,
        args: Value,
        ctx: &McpContext,
    ) -> Result<Value, String> {
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

        let proposals = extract_proposals(&raw)?;
        if proposals.is_empty() {
            return Err("invalid proposal: empty".into());
        }

        let mut results = Vec::new();
        for proposal in proposals {
            results.push(self.commit_proposal(proposal, ctx).await?);
        }
        if results.len() == 1 {
            Ok(results.remove(0))
        } else {
            Ok(json!({
                "status": "multi",
                "results": results,
                "message": "Multiple proposals committed — approve/exec each in order",
            }))
        }
    }

    async fn assist_side_effect(
        &self,
        name: &str,
        args: Value,
        ctx: &McpContext,
    ) -> Result<Value, String> {
        let tool_ctx = ToolContext {
            rpc_url: ctx.rpc_url.clone(),
            chain_id: ctx.chain_id,
            active_address: ctx.active_address,
        };
        self.assist
            .execute(name, args, &tool_ctx)
            .await
            .map_err(agent_err)
    }

    async fn commit_proposal(
        &self,
        proposal: TxProposal,
        ctx: &McpContext,
    ) -> Result<Value, String> {
        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "session_required: unlock Vaughan or run vaughan serve before proposing writes"
                    .to_string()
            })?;

        match try_propose_live(&session, &ctx.source, &proposal).await {
            Ok(Some(data)) => return Ok(data),
            Ok(None) => {}
            Err(e) if e.contains("wallet is locked") || e.contains("tui_offline") => {}
            Err(e) => return Err(e),
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
        match queue.lookup_status(proposal_id, session.as_bytes()) {
            Ok(status) => Ok(proposal_status_json(proposal_id, &status)),
            Err(e) => Ok(json!({
                "proposal_id": proposal_id,
                "status": "unknown",
                "error": e.code(),
            })),
        }
    }

    fn list_pending_proposals(&self, _ctx: &McpContext) -> Result<Value, String> {
        let queue = ProposalQueue::new(&self.profile_dir);
        let pending = queue.list_pending().map_err(|e| e.to_string())?;
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

    async fn control_plane_status(&self, _ctx: &McpContext) -> Result<Value, String> {
        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .unwrap_or_default();
        let has_session_file = !session.is_empty();
        let (reachable, unlocked, address) = if has_session_file {
            let reachable = ping(&session).await;
            match try_get_session(&session).await {
                Ok(Some(info)) => (reachable, true, Some(info.address)),
                Ok(None) => (false, false, None),
                Err(_) => (reachable, false, None),
            }
        } else {
            (false, false, None)
        };
        let profile_name = self
            .profile_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("default");
        let auto_exec = vaughan_core::core::is_sentient_profile(profile_name);
        Ok(json!({
            "control_plane_reachable": reachable,
            "session_file_present": has_session_file,
            "wallet_unlocked": unlocked,
            "active_address": address.map(|a| format!("{a:#x}")),
            "profile": profile_name,
            "sentient_auto_exec": auto_exec,
            "ready_for_writes": reachable && unlocked,
            "hint": if !reachable {
                "Start `vaughan --profile <name> serve --password-env …` or unlock the TUI"
            } else if !unlocked {
                "Control plane is up but wallet is locked — unlock or restart serve"
            } else if auto_exec {
                "Sentient profile: proposes auto-exec under policy"
            } else {
                "Adviser profile: proposes need human approval in TUI"
            },
        }))
    }

    async fn stealth_uri(&self, _ctx: &McpContext) -> Result<Value, String> {
        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .ok_or_else(|| "wallet_locked: unlock Vaughan or run vaughan serve".to_string())?;
        match try_stealth_uri(&session).await {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err("tui_offline: unlock Vaughan or run vaughan serve".into()),
            Err(e) => Err(e),
        }
    }

    async fn stealth_scan(&self, _ctx: &McpContext) -> Result<Value, String> {
        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .ok_or_else(|| "wallet_locked: unlock Vaughan or run vaughan serve".to_string())?;
        match try_stealth_scan(&session).await {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err("tui_offline: unlock Vaughan or run vaughan serve".into()),
            Err(e) => Err(e),
        }
    }

    async fn stealth_sweep(&self, args: Value, _ctx: &McpContext) -> Result<Value, String> {
        let addr = args
            .get("stealth_address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing stealth_address".to_string())?;
        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .ok_or_else(|| "wallet_locked: unlock Vaughan or run vaughan serve".to_string())?;
        match try_stealth_sweep(&session, addr).await {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err("tui_offline: unlock Vaughan or run vaughan serve".into()),
            Err(e) => Err(e),
        }
    }
}

fn agent_err(e: AgentError) -> String {
    match e {
        AgentError::InvalidToolCall(msg) => format!("invalid_tool_call: {msg}"),
        other => other.to_string(),
    }
}

/// Single `TxProposal` JSON, or multi envelopes like stealth (`pay_proposal` + `announce_proposal`).
fn extract_proposals(raw: &Value) -> Result<Vec<TxProposal>, String> {
    if let Ok(one) = serde_json::from_value::<TxProposal>(raw.clone()) {
        return Ok(vec![one]);
    }
    let mut out = Vec::new();
    for key in ["pay_proposal", "announce_proposal", "proposal"] {
        if let Some(v) = raw.get(key) {
            let p: TxProposal = serde_json::from_value(v.clone())
                .map_err(|e| format!("invalid proposal.{key}: {e}"))?;
            out.push(p);
        }
    }
    if out.is_empty() {
        return Err(format!(
            "invalid proposal: expected TxProposal or multi envelope ({raw})"
        ));
    }
    Ok(out)
}
