//! Tool dispatch for MCP — maps tool names to `vaughan-agent` registry.

use std::path::PathBuf;
use std::str::FromStr;

use alloy::primitives::{Address, U256};
use serde_json::{json, Value};
use vaughan_agent::paths::profile_dir;
use vaughan_agent::tools::{
    default_assist_registry_for, default_sensory_registry, ToolContext, ToolRegistry,
};
use vaughan_agent::AgentError;
use vaughan_core::chains::evm::adapter::EvmAdapter;
use vaughan_core::chains::evm::networks::{
    get_network_by_chain_id, get_network_by_id, resolve_rpc_endpoints,
};
use vaughan_core::core::persistence::StateManager;
use vaughan_core::core::proposal::{
    guard_mainnet_write, proposal_status_json, McpSessionToken, ProposalQueue, ProposalType,
    TxProposal,
};
use vaughan_core::core::{SignerClient, SignerIntent, SwapFamily};

use crate::browser_bridge::{self, browser_tool_definitions};
use crate::client::{
    ping, try_get_session, try_proposal_status, try_propose_live, try_stealth_scan,
    try_stealth_sweep, try_stealth_uri,
};
use crate::session_bridge::session_bridge_tool_definitions;

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
        Ok(Self::for_profile_dir(profile_dir))
    }

    pub fn for_profile_dir(profile_dir: PathBuf) -> Self {
        Self {
            sensory: default_sensory_registry(),
            assist: default_assist_registry_for(Some(&profile_dir)),
            profile_dir,
        }
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
        tools.extend(session_bridge_tool_definitions());
        tools.extend(browser_tool_definitions());
        tools
    }

    pub async fn call_tool(
        &self,
        name: &str,
        args: Value,
        ctx: &McpContext,
    ) -> Result<Value, String> {
        let profile_name = if ctx.profile.is_empty() {
            self.profile_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            ctx.profile.as_str()
        };

        if vaughan_core::core::is_sentient_profile(profile_name) {
            match name {
                "propose_contract_call"
                | "propose_lp_deploy"
                | "propose_v3_mint"
                | "propose_v3_increase"
                | "propose_v3_decrease"
                | "propose_v3_collect"
                | "propose_v3_transfer_position"
                | "propose_token_launch"
                | "propose_bridge"
                | "propose_hex_stake"
                | "propose_policy"
                | "get_stealth_uri"
                | "scan_stealth_notes"
                | "sweep_stealth_note" => {
                    return Err(format!(
                        "intent_untyped: {name} requires human approval via an adviser profile (not available on sentient)"
                    ));
                }
                _ => {}
            }
        }

        let ctx = self.refresh_context(ctx).await;
        match name {
            "get_proposal_status" => self.get_proposal_status(args, &ctx).await,
            "list_pending_proposals" => self.list_pending_proposals(&ctx),
            "get_control_plane_status" => self.control_plane_status(&ctx).await,
            "budget_status" => self.budget_status(&ctx).await,
            "import_token" => self.assist_side_effect("import_token", args, &ctx).await,
            "get_stealth_uri" => {
                self.require_power_unlocked(&ctx).await?;
                self.stealth_uri(&ctx).await
            }
            "scan_stealth_notes" => {
                self.require_power_unlocked(&ctx).await?;
                self.stealth_scan(&ctx).await
            }
            "sweep_stealth_note" => {
                self.require_power_unlocked(&ctx).await?;
                self.stealth_sweep(args, &ctx).await
            }
            name if name.starts_with("browser_") => {
                self.require_power_unlocked(&ctx).await?;
                match name {
                    "browser_open" => browser_bridge::browser_open(args, &ctx).await,
                    "browser_open_agg" => browser_bridge::browser_open_agg(args, &ctx).await,
                    "browser_navigate" => browser_bridge::browser_navigate(args, &ctx).await,
                    "browser_status" => browser_bridge::browser_status(&ctx).await,
                    "browser_snapshot" => browser_bridge::browser_snapshot(args, &ctx).await,
                    "browser_read_quote" => browser_bridge::browser_read_quote(args, &ctx).await,
                    "browser_click" => browser_bridge::browser_click(args, &ctx).await,
                    "browser_click_text" => browser_bridge::browser_click_text(args, &ctx).await,
                    "browser_type" => browser_bridge::browser_type(args, &ctx).await,
                    "browser_select_token" => {
                        browser_bridge::browser_select_token(args, &ctx).await
                    }
                    "browser_setup_swap" => browser_bridge::browser_setup_swap(args, &ctx).await,
                    "browser_submit_swap" => browser_bridge::browser_submit_swap(args, &ctx).await,
                    "browser_connect_wallet" => {
                        browser_bridge::browser_connect_wallet(args, &ctx).await
                    }
                    "browser_press" => browser_bridge::browser_press(args, &ctx).await,
                    "browser_wait" => browser_bridge::browser_wait(args, &ctx).await,
                    _ => Err(format!("unknown tool: {name}")),
                }
            }
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
            profile_dir: Some(self.profile_dir.clone()),
        };

        match name {
            "get_network" => {
                let mut out = json!({
                    "network_id": ctx.network_id,
                    "chain_id": ctx.chain_id,
                    "is_testnet": ctx.is_testnet,
                    // Redacted: RPC URLs can carry API keys in path/query —
                    // agents need the endpoint identity, not the credential.
                    "rpc_url": redact_rpc_url(&ctx.rpc_url),
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
                let profile_name = if ctx.profile.is_empty() {
                    self.profile_dir
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                } else {
                    ctx.profile.as_str()
                };
                if vaughan_core::core::is_sentient_profile(profile_name) {
                    let client = SignerClient::default();
                    let resp = client.status().await?;
                    let addr_str = resp.address.ok_or_else(|| {
                        "wallet_locked: signer daemon returned no active address".to_string()
                    })?;
                    return Ok(json!({
                        "address": addr_str,
                        "account_index": 0,
                        "account_label": "sentient",
                    }));
                }
                let addr = ctx.active_address.ok_or_else(|| {
                    "wallet_locked: no unlocked vault for this profile — the operator must run `vaughan serve --profile <name> --password-env <VAR>` (headless) or keep the TUI unlocked. Read-only tools also accept `account_address`; write tools do not.".to_string()
                })?;
                let session = McpSessionToken::read(&self.profile_dir)
                    .map_err(|e| e.user_message())?
                    .unwrap_or_default();
                let account_meta = if session.is_empty() {
                    None
                } else {
                    try_get_session(&session).await.ok().flatten()
                };
                Ok(json!({
                    "address": format!("{addr:#x}"),
                    "account_index": account_meta.as_ref().map(|s| s.account_index),
                    "account_label": account_meta.as_ref().map(|s| s.account_label.as_str()),
                }))
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
        let profile_name = if ctx.profile.is_empty() {
            self.profile_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            ctx.profile.as_str()
        };
        if vaughan_core::core::is_sentient_profile(profile_name) {
            let client = SignerClient::default();
            if let Ok(resp) = client.status().await {
                if let Some(addr_str) = resp.address {
                    if let Ok(addr) = Address::from_str(&addr_str) {
                        out.active_address = Some(addr);
                    }
                }
                if let Some(chain_id) = resp.chain_id {
                    out.chain_id = chain_id;
                    if let Some(net) = get_network_by_chain_id(chain_id) {
                        out.network_id = net.id.clone();
                        let persisted = StateManager::network_rpc_primary_for_profile(
                            &ctx.profile,
                            &out.network_id,
                        );
                        let (rpc_url, _) = resolve_rpc_endpoints(&net, persisted.as_deref(), None);
                        out.rpc_url = rpc_url;
                        out.is_testnet = net.is_testnet;
                    }
                }
            }
            return out;
        }

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
            let persisted =
                StateManager::network_rpc_primary_for_profile(&ctx.profile, &out.network_id);
            let (rpc_url, _) = resolve_rpc_endpoints(&net, persisted.as_deref(), None);
            out.rpc_url = rpc_url;
            out.is_testnet = net.is_testnet;
        }
        out
    }

    async fn list_assets(&self, ctx: &McpContext) -> Result<Value, String> {
        let addr = ctx.active_address.ok_or_else(|| {
            "wallet_locked: no unlocked vault for this profile — the operator must run `vaughan serve --profile <name> --password-env <VAR>` (headless) or keep the TUI unlocked. Read-only tools also accept `account_address`; write tools do not.".to_string()
        })?;
        let net = get_network_by_id(&ctx.network_id)
            .ok_or_else(|| format!("unknown network: {}", ctx.network_id))?;
        let persisted =
            StateManager::network_rpc_primary_for_profile(&ctx.profile, &ctx.network_id);
        let (primary, fallbacks) = resolve_rpc_endpoints(&net, persisted.as_deref(), None);
        let adapter = EvmAdapter::new(&primary, ctx.chain_id, &net.name, &fallbacks)
            .await
            .map_err(|e| e.to_string())?;
        let assets = adapter
            .get_assets(&format!("{addr:#x}"), &[])
            .await
            .map_err(|e| e.to_string())?;
        let rows: Vec<_> = assets
            .iter()
            .map(|bal| {
                let mut row = json!({
                    "symbol": bal.token.symbol,
                    "formatted": bal.formatted,
                    "contract": bal.token.contract_address,
                });
                if let Some(contract) = bal.token.contract_address.as_deref() {
                    if let Some(label) =
                        vaughan_core::core::token_origin_lookup_str(ctx.chain_id, contract)
                    {
                        let label = label.to_label();
                        if let Some(obj) = row.as_object_mut() {
                            obj.insert("display_symbol".into(), json!(label.display_symbol));
                            obj.insert("token_origin".into(), json!(label.token_origin));
                            if let Some(w) = label.warning {
                                obj.insert("warning".into(), json!(w));
                            }
                        }
                    }
                }
                row
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
        guard_mainnet_write(ctx.is_testnet).map_err(|e| e.to_string())?;
        self.require_power_unlocked(ctx).await?;

        let profile_name = if ctx.profile.is_empty() {
            self.profile_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            ctx.profile.as_str()
        };

        if vaughan_core::core::is_sentient_profile(profile_name) {
            let intent = map_sentient_tool_to_intent(name, &args, ctx)?;
            let client = SignerClient::default();
            let tx_hash = client.execute(&intent).await?;
            return Ok(json!({
                "status": "approved",
                "tx_hash": tx_hash,
            }));
        }

        let tool_ctx = ToolContext {
            rpc_url: ctx.rpc_url.clone(),
            chain_id: ctx.chain_id,
            active_address: ctx.active_address,
            profile_dir: Some(self.profile_dir.clone()),
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

    /// When an entitlement gate is on, refuse power tools without a qualifying
    /// WZRD holding (preferred) or legacy burn.
    async fn require_power_unlocked(&self, ctx: &McpContext) -> Result<(), String> {
        use vaughan_core::core::{
            assist_any_gate_enabled, entitlement_chain_id, require_power_features,
        };
        if !assist_any_gate_enabled() {
            return Ok(());
        }
        let Some(chain_id) = entitlement_chain_id() else {
            return Err(
                "assist_locked: WZRD entitlement chain unavailable — the gate cannot unlock".into(),
            );
        };
        // Vault-wide: positive cache for this chain unlocks every account. Cold
        // scan uses the session address; TUI re-verify writes the vault cache.
        let addr = ctx.active_address.ok_or_else(|| {
            "wallet_locked: no unlocked vault for this profile — the operator must run `vaughan serve --profile <name> --password-env <VAR>` (headless) or keep the TUI unlocked. Read-only tools also accept `account_address`; write tools do not.".to_string()
        })?;
        require_power_features(Some(&self.profile_dir), chain_id, &[addr])
            .await
            .map_err(|e| e.user_message())
    }

    /// `import_token` mutates persisted wallet state (the token list shown in
    /// the UI), so it requires a live wallet session — an MCP host with no
    /// unlocked wallet must not plant token entries into a profile.
    async fn assist_side_effect(
        &self,
        name: &str,
        args: Value,
        ctx: &McpContext,
    ) -> Result<Value, String> {
        let profile_name = if ctx.profile.is_empty() {
            self.profile_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            ctx.profile.as_str()
        };
        if vaughan_core::core::is_sentient_profile(profile_name) {
            let client = SignerClient::default();
            if !client.ping().await {
                return Err(
                    "session_required: signer daemon unreachable — start vaughan-signerd".into(),
                );
            }
            let tool_ctx = ToolContext {
                rpc_url: ctx.rpc_url.clone(),
                chain_id: ctx.chain_id,
                active_address: ctx.active_address,
                profile_dir: Some(self.profile_dir.clone()),
            };
            return self
                .assist
                .execute(name, args, &tool_ctx)
                .await
                .map_err(agent_err);
        }

        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "session_required: unlock Vaughan or run vaughan serve before import_token"
                    .to_string()
            })?;
        if try_get_session(&session).await.ok().flatten().is_none() {
            return Err(
                "session_required: control plane unreachable or wallet locked — unlock Vaughan first"
                    .to_string(),
            );
        }
        let tool_ctx = ToolContext {
            rpc_url: ctx.rpc_url.clone(),
            chain_id: ctx.chain_id,
            active_address: ctx.active_address,
            profile_dir: Some(self.profile_dir.clone()),
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

        // LP Brew steps: always enqueue so MCP returns immediately and the TUI
        // surfaces the card via poll_file_queue (multi-step pipeline UX).
        // Standalone V3 mint proposals use the same path (avoid 120s live-propose timeout).
        let file_queue_immediately =
            matches!(proposal.proposal_type, ProposalType::LpDeployStep { .. })
                || matches!(
                    &proposal.proposal_type,
                    ProposalType::ContractCall {
                        function_name: Some(name),
                        ..
                    } if name == "mint"
                );

        if !file_queue_immediately {
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
        let profile_name = self
            .profile_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("default");
        let auto_exec = vaughan_core::core::is_sentient_profile(profile_name);
        let autonomy = StateManager::agent_autonomy_tier_for_profile(profile_name);

        if auto_exec {
            let client = SignerClient::default();
            let (reachable, address) = match client.status().await {
                Ok(resp) => {
                    let addr = resp.address.and_then(|s| Address::from_str(&s).ok());
                    (true, addr)
                }
                Err(_) => (false, None),
            };
            return Ok(json!({
                "control_plane_reachable": reachable,
                "session_file_present": false,
                "wallet_unlocked": reachable,
                "agent_browser_control": StateManager::agent_browser_control_for_profile(profile_name),
                "agent_autonomy_tier": autonomy.as_str(),
                "operator_auto_connect": autonomy == vaughan_core::core::AgentAutonomyTier::Operator,
                "active_address": address.map(|a| format!("{a:#x}")),
                "active_account_index": if reachable { Some(0) } else { None },
                "active_account_label": if reachable { Some("sentient".to_string()) } else { None },
                "profile": profile_name,
                "sentient_auto_exec": true,
                "ready_for_writes": reachable,
                "hint": if reachable {
                    "Sentient profile: proposes auto-exec under signer policy via socket"
                } else {
                    "Signer daemon unreachable — run systemctl start vaughan-signerd"
                },
            }));
        }

        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .unwrap_or_default();
        let has_session_file = !session.is_empty();
        let (reachable, unlocked, address, account_index, account_label) = if has_session_file {
            let reachable = ping(&session).await;
            match try_get_session(&session).await {
                // A answered Session query proves reachability even if the
                // (earlier, 2s) ping timed out.
                Ok(Some(info)) => (
                    true,
                    true,
                    Some(info.address),
                    Some(info.account_index),
                    Some(info.account_label),
                ),
                // Locked/offline session: preserve the ping result — reporting
                // unreachable when the plane is actually up misleads agents
                // into spawning duplicate serve processes.
                Ok(None) => (reachable, false, None, None, None),
                Err(_) => (reachable, false, None, None, None),
            }
        } else {
            (false, false, None, None, None)
        };
        Ok(json!({
            "control_plane_reachable": reachable,
            "session_file_present": has_session_file,
            "wallet_unlocked": unlocked,
            "agent_browser_control": StateManager::agent_browser_control_for_profile(profile_name),
            "agent_autonomy_tier": autonomy.as_str(),
            "operator_auto_connect": autonomy == vaughan_core::core::AgentAutonomyTier::Operator,
            "active_address": address.map(|a| format!("{a:#x}")),
            "active_account_index": account_index,
            "active_account_label": account_label,
            "profile": profile_name,
            "sentient_auto_exec": auto_exec,
            "ready_for_writes": reachable && unlocked,
            "hint": if !reachable {
                "Start `vaughan --profile <name> serve --password-env …` or unlock the TUI"
            } else if !unlocked {
                "Control plane is up but wallet is locked — unlock or restart serve"
            } else if auto_exec {
                "Sentient profile: proposes auto-exec under policy"
            } else if autonomy == vaughan_core::core::AgentAutonomyTier::Operator {
                "Operator tier: auto-connect on allowlisted dApps; sign/propose still manual"
            } else {
                "Adviser profile: proposes need human approval in TUI"
            },
        }))
    }

    async fn budget_status(&self, ctx: &McpContext) -> Result<Value, String> {
        let profile_name = if ctx.profile.is_empty() {
            self.profile_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            ctx.profile.as_str()
        };

        if !vaughan_core::core::is_sentient_profile(profile_name) {
            return Ok(json!({
                "profile": profile_name,
                "enforcement": "n/a",
                "assets": [],
                "note": "adviser profile — proposals need human approval",
            }));
        }

        let client = SignerClient::default();
        let resp = client.status().await?;
        if let Some(mut budget) = resp.budget {
            budget["profile"] = json!(profile_name);
            budget["chain_id"] = json!(ctx.chain_id);
            return Ok(budget);
        }

        let ledger_path = format!(
            "{}/sentient-spend.json",
            resp.store.as_deref().unwrap_or("/var/lib/vaughan/sentient")
        );
        Ok(json!({
            "profile": profile_name,
            "chain_id": resp.chain_id.unwrap_or(ctx.chain_id),
            "enforcement": "enforced",
            "daily_cap_pct": 100,
            "window_reset_unix": 0,
            "assets": [],
            "allow_transfer_to": [],
            "ledger_path": ledger_path,
            "note": "no spend recorded this window",
            "active_address": resp.address,
        }))
    }

    async fn stealth_uri(&self, ctx: &McpContext) -> Result<Value, String> {
        let profile_name = if ctx.profile.is_empty() {
            self.profile_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            ctx.profile.as_str()
        };
        if vaughan_core::core::is_sentient_profile(profile_name) {
            return Err("intent_untyped: get_stealth_uri requires human approval via an adviser profile (not available on sentient)".into());
        }
        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .ok_or_else(|| "wallet_locked: unlock Vaughan or run vaughan serve".to_string())?;
        match try_stealth_uri(&session).await {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err("tui_offline: unlock Vaughan or run vaughan serve".into()),
            Err(e) => Err(e),
        }
    }

    async fn stealth_scan(&self, ctx: &McpContext) -> Result<Value, String> {
        let profile_name = if ctx.profile.is_empty() {
            self.profile_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            ctx.profile.as_str()
        };
        if vaughan_core::core::is_sentient_profile(profile_name) {
            return Err("intent_untyped: scan_stealth_notes requires human approval via an adviser profile (not available on sentient)".into());
        }
        let session = McpSessionToken::read(&self.profile_dir)
            .map_err(|e| e.user_message())?
            .ok_or_else(|| "wallet_locked: unlock Vaughan or run vaughan serve".to_string())?;
        match try_stealth_scan(&session).await {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err("tui_offline: unlock Vaughan or run vaughan serve".into()),
            Err(e) => Err(e),
        }
    }

    async fn stealth_sweep(&self, args: Value, ctx: &McpContext) -> Result<Value, String> {
        let profile_name = if ctx.profile.is_empty() {
            self.profile_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            ctx.profile.as_str()
        };
        if vaughan_core::core::is_sentient_profile(profile_name) {
            return Err("intent_untyped: sweep_stealth_note requires human approval via an adviser profile (not available on sentient)".into());
        }
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

/// Strip an RPC URL down to `scheme://host[:port]` for display: provider URLs
/// routinely embed API keys in the path or query (`/v3/<key>`, `?key=…`) and
/// tool results land in agent transcripts.
fn redact_rpc_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(u) => {
            let mut out = format!("{}://{}", u.scheme(), u.host_str().unwrap_or("<redacted>"));
            if let Some(port) = u.port() {
                out.push_str(&format!(":{port}"));
            }
            out
        }
        Err(_) => "<unparseable rpc url>".to_string(),
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

/// Map proposal tool arguments directly to a typed [`SignerIntent`] for sentient profiles.
///
/// Untyped tools return `intent_untyped` naming the adviser/human confirmation path
/// and never construct calldata or communicate with the signer.
pub fn map_sentient_tool_to_intent(
    name: &str,
    args: &Value,
    ctx: &McpContext,
) -> Result<SignerIntent, String> {
    match name {
        "propose_transfer" => {
            let recipient_str = args
                .get("recipient")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'recipient'".to_string())?;
            let recipient = Address::from_str(recipient_str)
                .map_err(|e| format!("invalid_tool_call: invalid recipient: {e}"))?;
            let amount_str = args
                .get("amount")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'amount'".to_string())?;
            let amount = U256::from_str(amount_str)
                .map_err(|e| format!("invalid_tool_call: invalid amount: {e}"))?;
            let token = args
                .get("token_address")
                .or_else(|| args.get("token"))
                .and_then(|v| v.as_str())
                .map(Address::from_str)
                .transpose()
                .map_err(|e| format!("invalid_tool_call: invalid token_address: {e}"))?;
            Ok(SignerIntent::Transfer {
                chain_id: ctx.chain_id,
                to: recipient,
                token,
                amount,
            })
        }
        "propose_swap" => {
            let router_str = args
                .get("router_address")
                .or_else(|| args.get("router"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'router_address'".to_string())?;
            let router = Address::from_str(router_str)
                .map_err(|e| format!("invalid_tool_call: invalid router_address: {e}"))?;
            let path_arr = args
                .get("path")
                .and_then(|v| v.as_array())
                .ok_or_else(|| "invalid_tool_call: missing 'path' array".to_string())?;
            let mut path = Vec::new();
            for p in path_arr {
                let s = p
                    .as_str()
                    .ok_or_else(|| "invalid_tool_call: path element must be a string".to_string())?;
                path.push(
                    Address::from_str(s)
                        .map_err(|e| format!("invalid_tool_call: invalid path address {s}: {e}"))?,
                );
            }
            let amount_in_str = args
                .get("amount_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'amount_in'".to_string())?;
            let amount_in = U256::from_str(amount_in_str)
                .map_err(|e| format!("invalid_tool_call: invalid amount_in: {e}"))?;
            let min_out_str = args
                .get("min_amount_out")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'min_amount_out'".to_string())?;
            let min_amount_out = U256::from_str(min_out_str)
                .map_err(|e| format!("invalid_tool_call: invalid min_amount_out: {e}"))?;
            Ok(SignerIntent::Swap {
                chain_id: ctx.chain_id,
                family: SwapFamily::V2,
                router,
                path,
                amount_in,
                min_amount_out,
                fee: None,
            })
        }
        "propose_v3_swap" => {
            let token_in_s = args
                .get("token_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'token_in'".to_string())?;
            let token_out_s = args
                .get("token_out")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'token_out'".to_string())?;
            let token_in = parse_v3_token(token_in_s, ctx.chain_id)?;
            let token_out = parse_v3_token(token_out_s, ctx.chain_id)?;

            let amount_in_str = args
                .get("amount_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'amount_in'".to_string())?;
            let amount_in = U256::from_str(amount_in_str)
                .map_err(|e| format!("invalid_tool_call: invalid amount_in: {e}"))?;

            let fee = args.get("fee").and_then(|v| v.as_u64()).map(|f| f as u32);
            let router = if let Some(r_str) = args
                .get("router")
                .or_else(|| args.get("router_address"))
                .and_then(|v| v.as_str())
            {
                Address::from_str(r_str)
                    .map_err(|e| format!("invalid_tool_call: invalid router: {e}"))?
            } else {
                let r_str = vaughan_core::core::deployment_for_chain(ctx.chain_id)
                    .map(|d| d.swap_router)
                    .ok_or_else(|| {
                        format!(
                            "invalid_tool_call: no V3 deployment for chain {}",
                            ctx.chain_id
                        )
                    })?;
                Address::from_str(r_str)
                    .map_err(|e| format!("invalid_tool_call: invalid deployment router: {e}"))?
            };

            let min_amount_out = if let Some(m_str) =
                args.get("min_amount_out").and_then(|v| v.as_str())
            {
                U256::from_str(m_str)
                    .map_err(|e| format!("invalid_tool_call: invalid min_amount_out: {e}"))?
            } else {
                U256::from(1)
            };

            Ok(SignerIntent::Swap {
                chain_id: ctx.chain_id,
                family: SwapFamily::V3,
                router,
                path: vec![token_in, token_out],
                amount_in,
                min_amount_out,
                fee,
            })
        }
        "propose_agg_swap" => {
            let token_in_s = args
                .get("token_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'token_in'".to_string())?;
            let token_out_s = args
                .get("token_out")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'token_out'".to_string())?;
            let token_in = parse_v3_token(token_in_s, ctx.chain_id)?;
            let token_out = parse_v3_token(token_out_s, ctx.chain_id)?;

            let amount_in_str = args
                .get("amount_in")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'amount_in'".to_string())?;
            let amount_in = U256::from_str(amount_in_str)
                .map_err(|e| format!("invalid_tool_call: invalid amount_in: {e}"))?;

            let min_amount_out = if let Some(m_str) =
                args.get("min_amount_out").and_then(|v| v.as_str())
            {
                U256::from_str(m_str)
                    .map_err(|e| format!("invalid_tool_call: invalid min_amount_out: {e}"))?
            } else {
                U256::from(1)
            };

            let router = if let Some(r_str) = args
                .get("router")
                .or_else(|| args.get("router_address"))
                .and_then(|v| v.as_str())
            {
                Address::from_str(r_str)
                    .map_err(|e| format!("invalid_tool_call: invalid router: {e}"))?
            } else {
                // Default to Squirrel on PulseChain
                Address::from_str("0xb5e0c5d63f25c788647087796d74ee176c1dc110")
                    .map_err(|e| format!("invalid_tool_call: {e}"))?
            };

            Ok(SignerIntent::Swap {
                chain_id: ctx.chain_id,
                family: SwapFamily::Agg,
                router,
                path: vec![token_in, token_out],
                amount_in,
                min_amount_out,
                fee: None,
            })
        }
        "propose_approve" => {
            let token_s = args
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'token'".to_string())?;
            let token = Address::from_str(token_s)
                .map_err(|e| format!("invalid_tool_call: invalid token: {e}"))?;
            let spender_s = args
                .get("spender")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'spender'".to_string())?;
            let spender = Address::from_str(spender_s)
                .map_err(|e| format!("invalid_tool_call: invalid spender: {e}"))?;
            let amount_s = args
                .get("amount")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'amount'".to_string())?;
            let amount = U256::from_str(amount_s)
                .map_err(|e| format!("invalid_tool_call: invalid amount: {e}"))?;
            let expiry = args.get("expiry").and_then(|v| v.as_u64());

            Ok(SignerIntent::Approve {
                chain_id: ctx.chain_id,
                token,
                spender,
                amount,
                expiry,
            })
        }
        "propose_wrap" => {
            let amount_s = args
                .get("amount")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'amount'".to_string())?;
            let amount = U256::from_str(amount_s)
                .map_err(|e| format!("invalid_tool_call: invalid amount: {e}"))?;
            Ok(SignerIntent::Wrap {
                chain_id: ctx.chain_id,
                amount,
            })
        }
        "propose_unwrap" => {
            let amount_s = args
                .get("amount")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "invalid_tool_call: missing 'amount'".to_string())?;
            let amount = U256::from_str(amount_s)
                .map_err(|e| format!("invalid_tool_call: invalid amount: {e}"))?;
            Ok(SignerIntent::Unwrap {
                chain_id: ctx.chain_id,
                amount,
            })
        }
        "propose_batch_transfer" | "propose_batch_7702" => {
            let calls_arr = args
                .get("calls")
                .or_else(|| args.get("legs"))
                .and_then(|v| v.as_array())
                .ok_or_else(|| "invalid_tool_call: missing 'calls' or 'legs'".to_string())?;
            let mut legs = Vec::new();
            for c in calls_arr {
                let to_s = c
                    .get("to")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "invalid_tool_call: leg missing 'to'".to_string())?;
                let to = Address::from_str(to_s)
                    .map_err(|e| format!("invalid_tool_call: invalid leg 'to': {e}"))?;
                let amt_s = c
                    .get("value_wei")
                    .or_else(|| c.get("amount"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                let amount = U256::from_str(amt_s)
                    .map_err(|e| format!("invalid_tool_call: invalid leg amount: {e}"))?;
                if let Some(d) = c.get("data").and_then(|v| v.as_str()) {
                    let d_clean = d.trim_start_matches("0x");
                    if !d_clean.is_empty() {
                        return Err("intent_untyped: batch leg contains calldata, which is not permitted on sentient profiles; use an adviser profile".to_string());
                    }
                }
                legs.push(vaughan_core::core::BatchTransferLeg { to, amount });
            }
            Ok(SignerIntent::BatchTransfer {
                chain_id: ctx.chain_id,
                legs,
            })
        }
        // Untyped tools: permanently refused on sentient, naming adviser/human approval
        _ => Err(format!(
            "intent_untyped: {name} requires human approval via an adviser profile (not available on sentient)"
        )),
    }
}

fn parse_v3_token(raw: &str, chain_id: u64) -> Result<Address, String> {
    let s = raw.trim();
    if s.eq_ignore_ascii_case("native")
        || s.eq_ignore_ascii_case("pls")
        || s.eq_ignore_ascii_case("eth")
    {
        return vaughan_core::core::dex_routers::wpls_for_chain(chain_id).ok_or_else(|| {
            format!("invalid_tool_call: no wrapped native token for chain {chain_id}")
        });
    }
    if s.eq_ignore_ascii_case("wpls") || s.eq_ignore_ascii_case("weth") {
        return vaughan_core::core::dex_routers::wpls_for_chain(chain_id).ok_or_else(|| {
            format!("invalid_tool_call: no wrapped native token for chain {chain_id}")
        });
    }
    if s.eq_ignore_ascii_case("wzrd") && chain_id == 943 {
        return Address::from_str(vaughan_core::core::wiz4rd::WZRD_SMOKE_943)
            .map_err(|e| format!("invalid_tool_call: invalid WZRD address: {e}"));
    }
    Address::from_str(s).map_err(|e| format!("invalid_tool_call: invalid token address '{s}': {e}"))
}
