//! `vaughan config …` — profile settings that need no vault unlock.
//!
//! Split out of `main.rs` so the binary's entry point stays a dispatch table:
//! these are five clap subcommand enums plus their handlers, and none of them
//! is reachable from any other command. The two wallet helpers at the bottom
//! are here for the same reason — `config network` is their only caller.
//!
//! Every handler here reads or writes profile METADATA (`wallet.json` fields
//! outside the encrypted vault). None of them unlocks, so none may touch key
//! material. The one exception to the "profile" part is `update-check`, which
//! is machine-wide and stored in its own file — see [`crate::update_check`].

use clap::Subcommand;
use serde_json::json;

use vaughan_core::core::{OperatingMode, StateManager, WalletState};

use crate::json_out;

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCmd {
    /// Loopback CDP for MCP browser_* tools (FR-7.5).
    AgentBrowser {
        #[command(subcommand)]
        action: AgentBrowserCmd,
    },
    /// Agent connect autonomy: advisor (manual) vs operator (auto on allowlist).
    AgentAutonomy {
        #[command(subcommand)]
        action: AgentAutonomyCmd,
    },
    /// Primary RPC URL per network (metadata only — no unlock).
    Rpc {
        #[command(subcommand)]
        action: RpcCmd,
    },
    /// Custom EVM networks (metadata only — no unlock).
    Network {
        #[command(subcommand)]
        action: NetworkCmd,
    },
    /// Periodic update check — off by default (no unprompted network calls).
    ///
    /// Unlike its siblings here, this one is machine-wide, not per-profile: it
    /// is about the installed binary, and it is stored outside `wallet.json`.
    UpdateCheck {
        #[command(subcommand)]
        action: UpdateCheckCmd,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum UpdateCheckCmd {
    /// Show whether the periodic check is enabled.
    Show,
    /// Allow one GitHub release lookup per 24h, reported on launch.
    On,
    /// Never check unless `vaughan update` is run explicitly (default).
    Off,
}

#[derive(Debug, Subcommand)]
pub(crate) enum NetworkCmd {
    /// List built-in and custom networks for this profile.
    List,
    /// Add a custom EVM network.
    Add {
        name: String,
        #[arg(long)]
        chain_id: u64,
        #[arg(long)]
        rpc_url: String,
        #[arg(long, default_value = "ETH")]
        symbol: String,
        #[arg(long)]
        testnet: bool,
    },
    /// Edit a custom network (id `custom-{chain_id}` or chain id).
    Edit {
        /// Network id (`custom-31337`) or chain id (`31337`).
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        rpc_url: Option<String>,
        #[arg(long)]
        symbol: Option<String>,
        #[arg(long)]
        testnet: Option<bool>,
    },
    /// Remove a custom network.
    Remove {
        /// Network id or chain id.
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RpcCmd {
    /// Show effective primary RPC for a network (default: active network).
    Show {
        #[arg(long)]
        network: Option<String>,
    },
    /// List known RPC presets for a network.
    List {
        #[arg(long)]
        network: Option<String>,
    },
    /// Set persisted primary RPC URL for a network.
    Set {
        url: String,
        #[arg(long)]
        network: Option<String>,
    },
    /// Clear override — use built-in default primary again.
    Reset {
        #[arg(long)]
        network: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentBrowserCmd {
    /// Show whether agent browser control is enabled.
    Show,
    /// Enable agent browser control (CDP default port when env unset).
    On,
    /// Disable agent browser control and clear vb.session.
    Off,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentAutonomyCmd {
    /// Show the current agent autonomy tier.
    Show,
    /// Manual Connect approval for every site (default).
    Advisor,
    /// Auto-connect on trusted dApp + Ag catalog hosts; never auto-sign.
    Operator,
}

pub(crate) fn run_config(
    profile: String,
    json_mode: bool,
    action: ConfigCmd,
) -> anyhow::Result<()> {
    match action {
        ConfigCmd::AgentBrowser { action } => match action {
            AgentBrowserCmd::Show => {
                let enabled = StateManager::agent_browser_control_for_profile(&profile);
                let data = json!({ "agent_browser_control": enabled, "profile": profile });
                json_out::print_json_value(json_mode, &data, || {
                    println!(
                        "agent_browser_control ({profile}): {}",
                        if enabled { "on" } else { "off" }
                    );
                });
            }
            AgentBrowserCmd::On => {
                StateManager::set_agent_browser_control_for_profile(&profile, true)?;
                let data = json!({ "agent_browser_control": true, "profile": profile });
                json_out::print_json_value(json_mode, &data, || {
                    println!("agent browser control enabled for profile `{profile}`");
                });
            }
            AgentBrowserCmd::Off => {
                StateManager::set_agent_browser_control_for_profile(&profile, false)?;
                let data = json!({ "agent_browser_control": false, "profile": profile });
                json_out::print_json_value(json_mode, &data, || {
                    println!("agent browser control disabled for profile `{profile}`");
                });
            }
        },
        ConfigCmd::AgentAutonomy { action } => match action {
            AgentAutonomyCmd::Show => {
                use vaughan_core::core::AgentAutonomyTier;
                let tier = StateManager::agent_autonomy_tier_for_profile(&profile);
                let data = json!({
                    "agent_autonomy_tier": tier.as_str(),
                    "operator_auto_connect": tier == AgentAutonomyTier::Operator,
                    "profile": profile,
                });
                json_out::print_json_value(json_mode, &data, || {
                    println!("agent_autonomy_tier ({profile}): {}", tier.as_str());
                });
            }
            AgentAutonomyCmd::Advisor => {
                use vaughan_core::core::AgentAutonomyTier;
                StateManager::set_agent_autonomy_tier_for_profile(
                    &profile,
                    AgentAutonomyTier::Advisor,
                )?;
                let data = json!({
                    "agent_autonomy_tier": "advisor",
                    "profile": profile,
                });
                json_out::print_json_value(json_mode, &data, || {
                    println!("agent autonomy tier set to advisor for profile `{profile}`");
                });
            }
            AgentAutonomyCmd::Operator => {
                use vaughan_core::core::AgentAutonomyTier;
                StateManager::set_agent_autonomy_tier_for_profile(
                    &profile,
                    AgentAutonomyTier::Operator,
                )?;
                let data = json!({
                    "agent_autonomy_tier": "operator",
                    "profile": profile,
                });
                json_out::print_json_value(json_mode, &data, || {
                    println!(
                        "agent autonomy tier set to operator for profile `{profile}` (auto-connect allowlist only)"
                    );
                });
            }
        },
        ConfigCmd::Rpc { action } => run_config_rpc(profile, json_mode, action)?,
        ConfigCmd::Network { action } => run_config_network(profile, json_mode, action)?,
        ConfigCmd::UpdateCheck { action } => {
            let set = |enabled: bool| -> anyhow::Result<()> {
                crate::update_check::set_enabled(enabled)?;
                // No profile in the message or the payload: this setting is
                // machine-wide, and naming a profile would imply otherwise.
                let data = json!({ "update_check": enabled, "scope": "machine" });
                json_out::print_json_value(json_mode, &data, || {
                    println!(
                        "periodic update check {} (applies to this install)",
                        if enabled { "enabled" } else { "disabled" }
                    );
                });
                Ok(())
            };
            match action {
                UpdateCheckCmd::Show => {
                    let enabled = crate::update_check::is_enabled();
                    let data = json!({ "update_check": enabled, "scope": "machine" });
                    json_out::print_json_value(json_mode, &data, || {
                        println!("update_check: {}", if enabled { "on" } else { "off" });
                        if !enabled {
                            println!(
                                "  no unprompted network calls; run `vaughan update` to check"
                            );
                        }
                    });
                }
                UpdateCheckCmd::On => set(true)?,
                UpdateCheckCmd::Off => set(false)?,
            }
        }
    }
    Ok(())
}

fn run_config_rpc(profile: String, json_mode: bool, action: RpcCmd) -> anyhow::Result<()> {
    use vaughan_core::chains::evm::networks::{get_network_by_id, resolve_rpc_endpoints};

    let sm = StateManager::for_profile(&profile).map_err(|e| anyhow::anyhow!("{e}"))?;
    let active_id = if sm.exists() {
        sm.load()?.active_network_id
    } else {
        "pulsechain".to_string()
    };
    let resolve_net = |network: &Option<String>| -> anyhow::Result<String> {
        Ok(network
            .clone()
            .unwrap_or_else(|| active_id.clone())
            .trim()
            .to_ascii_lowercase())
    };

    match action {
        RpcCmd::Show { network } => {
            let net_id = resolve_net(&network)?;
            let net = get_network_by_id(&net_id)
                .ok_or_else(|| anyhow::anyhow!("unknown network: {net_id}"))?;
            let persisted = StateManager::network_rpc_primary_for_profile(&profile, &net_id);
            let (primary, fallbacks) = resolve_rpc_endpoints(&net, persisted.as_deref(), None);
            let data = json!({
                "profile": profile,
                "network": net_id,
                "primary": primary,
                "fallbacks": fallbacks,
                "override": persisted,
            });
            json_out::print_json_value(json_mode, &data, || {
                println!("rpc ({profile} / {net_id}):");
                println!("  primary:   {primary}");
                for fb in &fallbacks {
                    println!("  fallback:  {fb}");
                }
            });
        }
        RpcCmd::List { network } => {
            let net_id = resolve_net(&network)?;
            let net = get_network_by_id(&net_id)
                .ok_or_else(|| anyhow::anyhow!("unknown network: {net_id}"))?;
            let presets: Vec<_> = net
                .known_rpc_endpoints()
                .iter()
                .map(|ep| json!({ "label": ep.label, "url": ep.url }))
                .collect();
            let data = json!({ "network": net_id, "presets": presets });
            json_out::print_json_value(json_mode, &data, || {
                println!("RPC presets for {net_id}:");
                for ep in net.known_rpc_endpoints() {
                    println!("  {:<14} {}", ep.label, ep.url);
                }
            });
        }
        RpcCmd::Set { url, network } => {
            let net_id = resolve_net(&network)?;
            get_network_by_id(&net_id)
                .ok_or_else(|| anyhow::anyhow!("unknown network: {net_id}"))?;
            StateManager::set_network_rpc_primary_for_profile(&profile, &net_id, Some(&url))?;
            let data = json!({ "network": net_id, "primary": url.trim() });
            json_out::print_json_value(json_mode, &data, || {
                println!("RPC primary set for `{net_id}`: {}", url.trim());
            });
        }
        RpcCmd::Reset { network } => {
            let net_id = resolve_net(&network)?;
            StateManager::set_network_rpc_primary_for_profile(&profile, &net_id, None)?;
            let data = json!({ "network": net_id, "reset": true });
            json_out::print_json_value(json_mode, &data, || {
                println!("RPC override cleared for `{net_id}` (built-in default restored).");
            });
        }
    }
    Ok(())
}

fn load_profile_wallet(profile: &str) -> anyhow::Result<WalletState> {
    let path = StateManager::profile_path(profile).map_err(|e| anyhow::anyhow!("{e}"))?;
    let wallet = WalletState::load_with_session(path, OperatingMode::HumanOnly, profile)?;
    if !wallet.is_initialized() {
        anyhow::bail!("no wallet for profile `{profile}` — run `vaughan create` first");
    }
    Ok(wallet)
}

fn resolve_custom_network_id(wallet: &WalletState, id: &str) -> anyhow::Result<String> {
    let needle = id.trim();
    if wallet.networks().get(needle).is_some() && wallet.networks().is_custom(needle) {
        return Ok(needle.to_ascii_lowercase());
    }
    if let Ok(chain_id) = needle.parse::<u64>() {
        let cid = format!("custom-{chain_id}");
        if wallet.networks().is_custom(&cid) {
            return Ok(cid);
        }
    }
    anyhow::bail!("custom network not found: {id}")
}

fn run_config_network(profile: String, json_mode: bool, action: NetworkCmd) -> anyhow::Result<()> {
    match action {
        NetworkCmd::List => {
            let wallet = load_profile_wallet(&profile)?;
            let nets: Vec<_> = wallet
                .networks()
                .networks()
                .iter()
                .map(|net| {
                    let (primary, fallbacks) = wallet.rpc_endpoints_for(net);
                    json!({
                        "id": net.id,
                        "name": net.name,
                        "chain_id": net.chain_id,
                        "custom": wallet.networks().is_custom(&net.id),
                        "testnet": net.is_testnet,
                        "rpc_url": primary,
                        "fallback_rpc_urls": fallbacks,
                    })
                })
                .collect();
            let data = json!({
                "profile": profile,
                "networks": nets,
                "active": wallet.networks().active_id(),
            });
            json_out::print_json_value(json_mode, &data, || {
                for net in wallet.networks().networks() {
                    let (primary, _) = wallet.rpc_endpoints_for(net);
                    let kind = if wallet.networks().is_custom(&net.id) {
                        "custom"
                    } else {
                        "built-in"
                    };
                    println!(
                        "{:<22} chain {:>6}  {:<8}  {}",
                        net.id, net.chain_id, kind, primary
                    );
                }
                println!("active: {}", wallet.networks().active_id());
            });
        }
        NetworkCmd::Add {
            name,
            chain_id,
            rpc_url,
            symbol,
            testnet,
        } => {
            let mut wallet = load_profile_wallet(&profile)?;
            let net = wallet
                .add_custom_network(&name, chain_id, &rpc_url, &symbol, testnet)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let data = json!({
                "profile": profile,
                "network": net,
            });
            json_out::print_json_value(json_mode, &data, || {
                println!(
                    "added custom network `{}` chain {} rpc {}",
                    net.name, net.chain_id, net.rpc_url
                );
            });
        }
        NetworkCmd::Edit {
            id,
            name,
            rpc_url,
            symbol,
            testnet,
        } => {
            let mut wallet = load_profile_wallet(&profile)?;
            let net_id = resolve_custom_network_id(&wallet, &id)?;
            let (cur_name, cur_rpc, cur_sym, cur_test) = {
                let current = wallet
                    .networks()
                    .get(&net_id)
                    .ok_or_else(|| anyhow::anyhow!("network not found: {net_id}"))?;
                (
                    current.name.clone(),
                    current.rpc_url.clone(),
                    current.native_symbol.clone(),
                    current.is_testnet,
                )
            };
            let updated = wallet
                .update_custom_network(
                    &net_id,
                    name.as_deref().unwrap_or(&cur_name),
                    rpc_url.as_deref().unwrap_or(&cur_rpc),
                    symbol.as_deref().unwrap_or(&cur_sym),
                    testnet.unwrap_or(cur_test),
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let data = json!({ "profile": profile, "network": updated });
            json_out::print_json_value(json_mode, &data, || {
                println!("updated `{}` — rpc {}", updated.name, updated.rpc_url);
            });
        }
        NetworkCmd::Remove { id } => {
            let mut wallet = load_profile_wallet(&profile)?;
            let net_id = resolve_custom_network_id(&wallet, &id)?;
            wallet
                .remove_custom_network(&net_id)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let data = json!({ "profile": profile, "removed": net_id });
            json_out::print_json_value(json_mode, &data, || {
                println!("removed custom network `{net_id}`");
            });
        }
    }
    Ok(())
}
