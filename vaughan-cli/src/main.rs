//! vaughan — Vaughan wallet: interactive TUI by default, scriptable subcommands.
//!
//! Run `vaughan` with no arguments to open the terminal wallet. Subcommands
//! (`send`, `balance`, `browse`, …) provide non-interactive vault access for
//! scripts and CI. All vault-touching subcommands require the wallet password
//! — via `--password-env NAME` for automation, or an interactive prompt.

mod json_out;
mod lp;
mod mcp_install;
mod serve;
pub mod signerd;

use std::path::PathBuf;
use std::str::FromStr;

use alloy::primitives::{Address, U256};
use clap::{Parser, Subcommand};
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use vaughan_agent::paths::profile_dir;
use vaughan_agent::tools::{default_assist_registry, ToolContext};
use vaughan_core::chains::ChainTransaction;
use vaughan_core::core::proposal::{ProposalQueue, TxProposal};
use vaughan_core::core::{
    guard_mainnet_write, reject_deferred_sentient_profile, OperatingMode, StateManager,
    TransactionService, WalletState,
};

#[derive(Debug, Parser)]
#[command(
    name = "vaughan",
    version,
    about = "Vaughan wallet — run with no args for the interactive TUI"
)]
struct Cli {
    /// Vault file path (default: the profile data dir).
    #[arg(long, global = true)]
    vault: Option<PathBuf>,

    /// Profile name (default: "default").
    #[arg(long, global = true, default_value = "default")]
    profile: String,

    /// Emit machine-readable JSON (`{ "ok": true, "data": … }`).
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the interactive terminal wallet (same as running `vaughan` alone).
    Tui,
    /// Broadcast a transaction: contract call (`--data`) or value transfer.
    ///
    /// Estimates the fee, signs with the unlocked vault, and broadcasts.
    /// Prints the tx hash. This is the deploy path: pass a contract's
    /// creation bytecode or a call's calldata as `--data`.
    Send {
        /// Destination address (contract or EOA). Omit for contract creation
        /// (creation bytecode in `--data`).
        to: Option<String>,
        /// Calldata hex (`0x...`); omit for a plain value transfer.
        #[arg(long)]
        data: Option<String>,
        /// Value in wei (decimal string).
        #[arg(long, default_value = "0")]
        value: String,
        /// Network id (built-in: pulsechain, pulsechain-testnet-v4).
        #[arg(long)]
        network: Option<String>,
        /// RPC url override (dev node, dedicated provider).
        #[arg(long)]
        rpc_url: Option<String>,
        /// Env var holding the wallet password (non-interactive).
        #[arg(long)]
        password_env: Option<String>,
        /// Broadcast without the interactive confirmation prompt (scripts).
        /// Without this flag the full request — recipient, value, chain, and
        /// estimated fee — is printed and must be confirmed with `y`.
        #[arg(long)]
        yes: bool,
    },
    /// Show the active account's native balance.
    Balance {
        /// Network id override.
        #[arg(long)]
        network: Option<String>,
        /// RPC url override (dev node, dedicated provider).
        #[arg(long)]
        rpc_url: Option<String>,
        /// Env var holding the wallet password (non-interactive).
        #[arg(long)]
        password_env: Option<String>,
    },
    /// Show every detected balance (native + known ERC-20s) — auto asset
    /// detection against the curated per-chain token list (EIP-20 balances,
    /// batched via Multicall3; see docs/optimizations.md).
    Assets {
        /// Network id override.
        #[arg(long)]
        network: Option<String>,
        /// RPC url override (dev node, dedicated provider).
        #[arg(long)]
        rpc_url: Option<String>,
        /// Env var holding the wallet password (non-interactive).
        #[arg(long)]
        password_env: Option<String>,
    },
    /// List built-in networks.
    Networks,
    /// Create a new wallet (prints the mnemonic once — store it securely).
    Create {
        /// Env var holding the wallet password (non-interactive).
        #[arg(long)]
        password_env: Option<String>,
    },
    /// Restore a wallet from a mnemonic phrase (entered via hidden prompt —
    /// never on the command line, where it would leak into shell history
    /// and the process list).
    Restore {
        /// Env var holding the wallet password (non-interactive).
        #[arg(long)]
        password_env: Option<String>,
    },
    /// Browse and inspect a smart contract, probe interface, or execute read-only calls.
    Browse {
        /// Contract address (0x...).
        address: String,
        /// Optional function name to call against verified ABI (e.g. `getReserves`, `name`).
        #[arg(long)]
        call: Option<String>,
        /// Optional arguments for the function call.
        #[arg(long, num_args = 1..)]
        args: Vec<String>,
        /// Optional raw calldata hex to execute as read-only eth_call.
        #[arg(long)]
        call_raw: Option<String>,
        /// Network id override (e.g. pulsechain, pulsechain-testnet-v4).
        #[arg(long)]
        network: Option<String>,
        /// RPC url override.
        #[arg(long)]
        rpc_url: Option<String>,
    },
    /// Sign EIP-712 typed data (eth_signTypedData_v4) without a dApp — JSON from `--data` or `--file`.
    SignTypedData {
        /// Typed-data JSON object (types, domain, primaryType, message).
        #[arg(long, conflicts_with = "file")]
        data: Option<String>,
        /// Path to a JSON file containing the typed-data payload.
        #[arg(long, conflicts_with = "data")]
        file: Option<PathBuf>,
        /// Env var holding the wallet password (non-interactive).
        #[arg(long)]
        password_env: Option<String>,
        /// Sign without the interactive confirmation prompt (scripts).
        #[arg(long)]
        yes: bool,
    },
    /// Draft a transaction proposal (does not sign — queues for TUI approval).
    Propose {
        #[command(subcommand)]
        action: ProposeCmd,
    },
    /// List or inspect pending MCP/CLI proposals.
    Proposals {
        #[command(subcommand)]
        action: ProposalsCmd,
    },
    /// MCP stdio server for external agents (Cursor, Claude Code, …).
    Mcp {
        #[command(subcommand)]
        action: Option<mcp_install::McpCmd>,
        /// Client label shown on approval cards (e.g. cursor, claude).
        #[arg(long, default_value = "cursor")]
        source: String,
    },
    /// Headless wallet daemon (v2): unlock profile and serve MCP control plane.
    Serve {
        /// Env var holding the vault password (required).
        #[arg(long)]
        password_env: Option<String>,
    },
    /// Isolated signer daemon: peer-UID-gated Unix domain socket for typed intents.
    Signerd {
        #[command(subcommand)]
        action: Option<SignerdCmd>,
        /// Store directory containing wallet.json, sentient-policy.toml, and spend ledger.
        #[arg(long)]
        store: Option<PathBuf>,
        /// Unix domain socket path.
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Peer UID permitted to connect (repeatable; default: invoking UID).
        #[arg(long = "allow-uid")]
        allow_uid: Vec<u32>,
        /// Env var holding the vault password (non-interactive).
        #[arg(long)]
        password_env: Option<String>,
        /// Explicit old profile directory override (tests).
        #[arg(long)]
        old_profile_dir: Option<PathBuf>,
    },
    /// Install a sentient skill+policy preset into the active profile.
    Preset {
        #[command(subcommand)]
        action: PresetCmd,
    },
    /// LP Brew: plan or enqueue deploy steps (Advisor TUI approval).
    Lp {
        #[command(subcommand)]
        action: LpCmd,
    },
    /// Profile configuration (metadata only — no vault unlock).
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
}

#[derive(Debug, Subcommand)]
enum ProposeCmd {
    /// Draft a native or ERC-20 transfer proposal.
    Transfer {
        recipient: String,
        /// Amount in base units (wei).
        amount: String,
        /// Optional ERC-20 token contract (omit for native).
        #[arg(long)]
        token: Option<String>,
        #[arg(long, default_value = "CLI transfer proposal")]
        explanation: String,
    },
    /// Draft a fixed-supply testnet ERC-20 deploy (meme-coin launcher).
    TokenLaunch {
        name: String,
        symbol: String,
        /// Human supply (18 decimals), e.g. 1000000
        supply: String,
        #[arg(long, default_value = "CLI token launch")]
        explanation: String,
    },
}

#[derive(Debug, Subcommand)]
enum ProposalsCmd {
    /// List pending proposals.
    List,
    /// Show one proposal by id.
    Show { proposal_id: String },
}

#[derive(Debug, Subcommand)]
enum LpCmd {
    /// Dry-run: show lifecycle branch and step list (no proposals).
    Plan {
        #[arg(long)]
        password_env: Option<String>,
        #[command(flatten)]
        brew: lp::LpBrewArgs,
    },
    /// Enqueue the first LP deploy step (requires unlocked TUI MCP session).
    Deploy {
        #[arg(long)]
        password_env: Option<String>,
        #[command(flatten)]
        brew: lp::LpBrewArgs,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCmd {
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
}

#[derive(Debug, Subcommand)]
enum NetworkCmd {
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
enum RpcCmd {
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
enum AgentBrowserCmd {
    /// Show whether agent browser control is enabled.
    Show,
    /// Enable agent browser control (CDP default port when env unset).
    On,
    /// Disable agent browser control and clear vb.session.
    Off,
}

#[derive(Debug, Subcommand)]
enum AgentAutonomyCmd {
    /// Show the current agent autonomy tier.
    Show,
    /// Manual Connect approval for every site (default).
    Advisor,
    /// Auto-connect on trusted dApp + Ag catalog hosts; never auto-sign.
    Operator,
}

#[derive(Debug, Subcommand)]
pub enum SignerdCmd {
    /// Migrate an existing profile into the signer-owned store.
    Migrate {
        /// Profile name to migrate (default: sentient).
        #[arg(long, default_value = "sentient")]
        profile: String,
        /// Explicit target store root (default: /var/lib/vaughan/<profile>).
        #[arg(long)]
        store_root: Option<PathBuf>,
        /// Explicit old profile directory override (tests).
        #[arg(long)]
        old_profile_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum PresetCmd {
    /// List bundled preset ids.
    List,
    /// Copy preset SKILL.md + policy into this profile (`--profile sentient`).
    Apply {
        /// e.g. balanced, high-risk-gambler, quant-risk-reward, cautious
        id: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let Cli {
        vault,
        profile,
        json,
        command,
    } = cli;
    let result = match command {
        None | Some(Command::Tui) => {
            vaughan_tui::run_interactive(&profile).map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(Command::Mcp { action, source }) => match action {
            Some(mcp_install::McpCmd::Install {
                client,
                profile: cmd_profile,
                dry_run,
            }) => {
                let prof = cmd_profile.as_deref().unwrap_or(&profile);
                mcp_install::run_install(client, prof, dry_run)
            }
            None => match reject_deferred_sentient_profile(&profile) {
                Err(e) => Err(anyhow::anyhow!("{}", e.user_message())),
                Ok(()) => {
                    vaughan_core::logging::init_logging();
                    let runtime =
                        tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
                    runtime
                        .block_on(vaughan_mcp::run_stdio_server(profile, source))
                        .map_err(|e| anyhow::anyhow!("{e}"))
                }
            },
        },
        Some(Command::Serve { password_env }) => {
            vaughan_core::logging::init_logging();
            let password_res = if vaughan_core::core::is_sentient_profile(&profile) {
                // Sentient serve is a thin client connecting to vaughan-signerd;
                // in-process wallet unlock is disabled and no password is required.
                Ok(secrecy::SecretString::from(String::new()))
            } else {
                serve::password_from_env(password_env.as_deref()).inspect(|_| {
                    if let Some(var) = password_env.as_deref() {
                        std::env::remove_var(var);
                    }
                })
            };
            match password_res {
                Err(e) => Err(e),
                Ok(password) => {
                    let runtime =
                        tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
                    runtime
                        .block_on(serve::run_serve(profile, password))
                        .map_err(|e| anyhow::anyhow!("{e}"))
                }
            }
        }
        Some(Command::Signerd {
            action,
            store,
            socket,
            allow_uid,
            password_env,
            old_profile_dir,
        }) => match action {
            Some(SignerdCmd::Migrate {
                profile: migrate_profile,
                store_root,
                old_profile_dir: migrate_old_dir,
            }) => signerd::migrate::run_migrate(
                &migrate_profile,
                store_root.as_deref(),
                migrate_old_dir.as_deref(),
            ),
            None => match (store, socket) {
                (Some(store), Some(socket)) => {
                    vaughan_core::logging::init_logging();
                    match prompt_password(password_env.as_deref()) {
                        Err(e) => Err(e),
                        Ok(password) => {
                            if let Some(var) = password_env.as_deref() {
                                std::env::remove_var(var);
                            }
                            let runtime =
                                tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
                            runtime
                                .block_on(signerd::run_signerd(
                                    store,
                                    socket,
                                    allow_uid,
                                    password,
                                    old_profile_dir,
                                ))
                                .map_err(|e| anyhow::anyhow!("{e}"))
                        }
                    }
                }
                _ => Err(anyhow::anyhow!(
                    "--store and --socket are required to run signerd daemon (or run `vaughan signerd migrate`)"
                )),
            },
        },
        Some(Command::Preset { action }) => run_preset(profile, json, action),
        Some(Command::Config { action }) => run_config(profile, json, action),
        Some(command) => {
            vaughan_core::logging::init_logging();
            let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            runtime.block_on(run_cli(vault, profile, json, command))
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run_cli(
    vault: Option<PathBuf>,
    profile: String,
    json_mode: bool,
    command: Command,
) -> anyhow::Result<()> {
    let path = if let Some(custom_vault) = vault {
        custom_vault
    } else {
        StateManager::profile_path(&profile).unwrap_or_else(|_| {
            eprintln!("error: could not resolve the profile vault path; pass --vault");
            std::process::exit(1);
        })
    };
    let mut wallet = WalletState::load_with_session(path, OperatingMode::HumanOnly, profile)?;

    match command {
        Command::Tui => unreachable!("tui handled in main"),
        Command::Networks => {
            let active_id = wallet.networks().active_id();
            let nets: Vec<_> = wallet
                .networks()
                .networks()
                .iter()
                .map(|net| {
                    let (primary, fallbacks) = wallet.rpc_endpoints_for(net);
                    json!({
                        "id": net.id,
                        "chain_id": net.chain_id,
                        "testnet": net.is_testnet,
                        "rpc_url": primary,
                        "fallback_rpc_urls": fallbacks,
                        "rpc_override": wallet.network_rpc_primary(&net.id),
                    })
                })
                .collect();
            let data = json!({
                "networks": nets,
                "active": active_id,
            });
            json_out::print_json_value(json_mode, &data, || {
                for net in wallet.networks().networks() {
                    let (primary, fallbacks) = wallet.rpc_endpoints_for(net);
                    println!(
                        "{:<24} chain {}  {}  {}",
                        net.id,
                        net.chain_id,
                        if net.is_testnet { "testnet" } else { "mainnet" },
                        primary
                    );
                    for fb in fallbacks {
                        println!("{:<24}   (fallback) {}", "", fb);
                    }
                }
                println!("active: {active_id}");
            });
        }
        Command::Create { password_env } => {
            if wallet.is_initialized() {
                anyhow::bail!("a wallet already exists at {}", wallet.path().display());
            }
            let password = prompt_password(password_env.as_deref())?;
            let mnemonic = vaughan_core::security::hd_wallet::generate_mnemonic()?;
            wallet.create(&password, mnemonic.clone())?;
            println!("wallet created at {}", wallet.path().display());
            println!("address: {}", wallet.active_address()?);
            // Mnemonic goes to stderr so stdout stays script-clean; it is
            // shown once and never stored unencrypted.
            eprintln!("\n⚠️  mnemonic (write this down on paper — shown only once, never stored unencrypted):");
            eprintln!("{mnemonic}");
        }
        Command::Restore { password_env } => {
            if wallet.is_initialized() {
                anyhow::bail!("a wallet already exists at {}", wallet.path().display());
            }
            let password = prompt_password(password_env.as_deref())?;
            // Hidden prompt (also reads piped stdin) — never an argv argument,
            // which would leak into shell history and the process list.
            let phrase = prompt_secret("recovery phrase: ")?;
            wallet.restore(&password, phrase.expose_secret())?;
            println!("wallet restored at {}", wallet.path().display());
            println!("address: {}", wallet.active_address()?);
        }
        Command::Balance {
            network,
            rpc_url,
            password_env,
        } => {
            unlock(&mut wallet, password_env.as_deref())?;
            if let Some(id) = network {
                wallet.set_active_network(&id)?;
            }
            if let Some(url) = rpc_url {
                wallet.set_rpc_override(&url);
            }
            let balance = wallet.balance().await?;
            let addr = wallet.active_address()?.to_string();
            let data = json!({
                "address": addr,
                "balance": balance.formatted,
                "symbol": balance.token.symbol,
            });
            json_out::print_json_value(json_mode, &data, || {
                println!("{addr}  ({} {})", balance.formatted, balance.token.symbol);
            });
        }
        Command::Assets {
            network,
            rpc_url,
            password_env,
        } => {
            unlock(&mut wallet, password_env.as_deref())?;
            if let Some(id) = network {
                wallet.set_active_network(&id)?;
            }
            if let Some(url) = rpc_url {
                wallet.set_rpc_override(&url);
            }
            let assets = wallet.assets().await?;
            let addr = wallet.active_address()?.to_string();
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
            let data = json!({ "address": addr, "assets": rows });
            json_out::print_json_value(json_mode, &data, || {
                println!("{addr}");
                if assets.is_empty() {
                    println!("no balances detected");
                }
                for bal in assets {
                    let contract = bal
                        .token
                        .contract_address
                        .as_deref()
                        .map(|a| format!("  {a}"))
                        .unwrap_or_default();
                    println!("{:<6} {}{contract}", bal.token.symbol, bal.formatted);
                }
            });
        }
        Command::Send {
            to,
            data,
            value,
            network,
            rpc_url,
            password_env,
            yes,
        } => {
            unlock(&mut wallet, password_env.as_deref())?;
            if let Some(id) = network {
                wallet.set_active_network(&id)?;
            }
            if let Some(url) = rpc_url {
                wallet.set_rpc_override(&url);
            }
            let data = data.as_deref().unwrap_or("");
            let to = match (to, data.is_empty()) {
                (Some(to), _) => to,
                (None, false) => {
                    // Contract creation: no recipient; the adapter fills the
                    // create address from the signed tx.
                    "0x0000000000000000000000000000000000000000".to_string()
                }
                (None, true) => {
                    anyhow::bail!("a recipient `TO` is required when `--data` is empty")
                }
            };
            let net = wallet.networks().active();
            let tx = TransactionService::new().build_contract_call(
                wallet.active_address()?,
                to,
                data,
                value,
                net.chain_id,
            )?;
            let ChainTransaction::Evm(evm) = tx else {
                anyhow::bail!("expected an EVM transaction");
            };
            // Show the full request — recipient, value, chain, and fee —
            // then require explicit confirmation unless `--yes` was passed.
            let fee_line = match wallet.estimate_transaction_fee(evm.clone()).await {
                Ok(fee) => format!("fee:     ~{} (est.)", fee.total),
                Err(_) => "fee:     unavailable (estimation failed)".to_string(),
            };
            eprintln!(
                "network: {} (chain {})\nfrom:    {}\nto:      {}\nvalue:   {} wei\n{fee_line}{}",
                net.name,
                net.chain_id,
                evm.from,
                evm.to,
                evm.value,
                evm.data
                    .as_deref()
                    .map(|d| format!("\ndata:    {d}"))
                    .unwrap_or_default()
            );
            if !yes && !confirm_broadcast()? {
                anyhow::bail!("aborted: transaction not confirmed");
            }
            let reserve_info = if vaughan_core::core::is_sentient_profile(wallet.profile_name()) {
                let profile_dir = vaughan_agent::paths::profile_dir(wallet.path());
                let bal = wallet.balance().await?;
                let key = gate_and_reserve_sentient_send(
                    &profile_dir,
                    net.chain_id,
                    &evm.to,
                    &evm.value,
                    &bal.raw,
                    evm.data.as_deref(),
                )?;
                Some((profile_dir, key))
            } else {
                None
            };
            let hash = match wallet.send_transaction(evm).await {
                Ok(h) => h,
                Err(e) => {
                    if let Some((dir, key)) = reserve_info {
                        if let Err(re) = vaughan_agent::sentient::release_legs(&dir, &key) {
                            tracing::warn!(
                                target: "vaughan_cli",
                                "failed to release spend reservation for {key}: {re}"
                            );
                        }
                    }
                    return Err(e.into());
                }
            };
            println!("{hash}");
        }
        Command::SignTypedData {
            data,
            file,
            password_env,
            yes,
        } => {
            if vaughan_core::core::is_sentient_profile(wallet.profile_name()) {
                anyhow::bail!(
                    "typed_data_not_permitted: signing authorisations is an adviser-path action"
                );
            }
            unlock(&mut wallet, password_env.as_deref())?;
            let typed_data = load_typed_data_json(data.as_deref(), file.as_deref())?;
            validate_typed_data_shape(&typed_data)?;
            let from = wallet.active_address()?.to_string();
            let primary = typed_data["primaryType"].as_str().unwrap_or("?");
            let domain_name = typed_data["domain"]["name"].as_str().unwrap_or("?");
            eprintln!(
                "method:  eth_signTypedData_v4\nfrom:    {from}\ntype:    {primary}\ndomain:  {domain_name}"
            );
            if !yes && !confirm_sign()? {
                anyhow::bail!("aborted: signature not confirmed");
            }
            let sig = wallet.sign_typed_data(&typed_data)?;
            let data_out = json!({
                "signature": sig,
                "address": from,
                "primaryType": primary,
            });
            json_out::print_json_value(json_mode, &data_out, || println!("{sig}"));
        }
        Command::Lp { action } => match action {
            LpCmd::Plan { password_env, brew } => {
                unlock(&mut wallet, password_env.as_deref())?;
                if let Some(id) = brew.network.as_deref() {
                    wallet.set_active_network(id)?;
                }
                if let Some(url) = brew.rpc_url.as_deref() {
                    wallet.set_rpc_override(url);
                }
                lp::run_lp_plan(&wallet, lp::LpPlanArgs { brew })
                    .await
                    .map_err(|e| anyhow::anyhow!(e.user_message()))?;
            }
            LpCmd::Deploy { password_env, brew } => {
                unlock(&mut wallet, password_env.as_deref())?;
                if let Some(id) = brew.network.as_deref() {
                    wallet.set_active_network(id)?;
                }
                if let Some(url) = brew.rpc_url.as_deref() {
                    wallet.set_rpc_override(url);
                }
                lp::run_lp_deploy(&wallet, wallet.path(), lp::LpDeployArgs { brew })
                    .await
                    .map_err(|e| anyhow::anyhow!(e.user_message()))?;
            }
        },
        Command::Browse {
            address,
            call,
            args,
            call_raw,
            network,
            rpc_url,
        } => {
            let addr: alloy::primitives::Address = address
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid target address: {e}"))?;

            if let Some(id) = network {
                wallet.set_active_network(&id)?;
            }
            if let Some(url) = rpc_url {
                wallet.set_rpc_override(&url);
            }

            let net = wallet.networks().active();
            let chain_id = net.chain_id;
            let adapter = wallet.active_adapter().await?;
            let engine = vaughan_core::browser::BrowserEngine::new();

            adapter
                .with_provider(|provider| {
                    let eng = engine.clone();
                    let c = call.clone();
                    let av = args.clone();
                    let cr = call_raw.clone();
                    async move {
                        if let Some(raw_hex) = cr {
                            let clean = raw_hex.trim().strip_prefix("0x").unwrap_or(raw_hex.trim());
                            let bytes = alloy::primitives::Bytes::from(
                                hex::decode(clean).map_err(|e| {
                                    vaughan_core::error::WalletError::InvalidTransaction(
                                        e.to_string(),
                                    )
                                })?,
                            );
                            let out = eng
                                .call_raw(&provider, addr, bytes)
                                .await
                                .map_err(vaughan_core::error::WalletError::RpcError)?;
                            let data = json!({
                                "address": addr.to_checksum(None),
                                "chain_id": chain_id,
                                "result_hex": format!("0x{}", hex::encode(&out)),
                            });
                            json_out::print_json_value(json_mode, &data, || {
                                println!("0x{}", hex::encode(&out));
                            });
                            return Ok(());
                        }

                        let insp = eng.inspect(&provider, chain_id, addr).await;
                        let mut data = json!({
                            "address": addr.to_checksum(None),
                            "chain_id": chain_id,
                            "network": net.name,
                            "fingerprint": format!("{:?}", insp.fingerprint),
                        });

                        match &insp.abi_resolution {
                            vaughan_core::browser::abi::AbiResolution::Verified(abi) => {
                                data["abi"] = json!({
                                    "status": "verified",
                                    "function_count": abi.functions.len(),
                                });
                                if let Some(fn_name) = c {
                                    let res = eng
                                        .call_named(&provider, addr, abi, &fn_name, &av)
                                        .await
                                        .map_err(vaughan_core::error::WalletError::RpcError)?;
                                    data["call"] = json!({
                                        "function": fn_name,
                                        "decoded": res.decoded_values,
                                        "result_hex": format!("0x{}", hex::encode(&res.raw_output)),
                                    });
                                    json_out::print_json_value(json_mode, &data, || {
                                        if res.decoded_values.is_empty() {
                                            println!(
                                                "Result:      0x{}",
                                                hex::encode(&res.raw_output)
                                            );
                                        } else {
                                            println!(
                                                "Result:      {}",
                                                res.decoded_values.join(", ")
                                            );
                                        }
                                    });
                                } else {
                                    let mut names: Vec<_> =
                                        abi.functions.keys().map(|k| k.as_str()).collect();
                                    names.sort_unstable();
                                    data["functions"] = json!(names);
                                    json_out::print_json_value(json_mode, &data, || {
                                        println!("Address:     {}", addr.to_checksum(None));
                                        println!("Chain:       {} ({})", net.name, chain_id);
                                        println!("Fingerprint: {:?}", insp.fingerprint);
                                        println!(
                                            "ABI:         Verified ({} functions)",
                                            abi.functions.len()
                                        );
                                        println!("Functions:   {}", names.join(", "));
                                    });
                                }
                            }
                            vaughan_core::browser::abi::AbiResolution::Unverified => {
                                let hex_list: Vec<_> = insp
                                    .candidate_selectors
                                    .iter()
                                    .map(|s| vaughan_core::browser::selectors::selector_to_hex(*s))
                                    .collect();
                                data["abi"] = json!({
                                    "status": "unverified",
                                    "candidate_selectors": hex_list,
                                });
                                json_out::print_json_value(json_mode, &data, || {
                                    println!("Address:     {}", addr.to_checksum(None));
                                    println!("Chain:       {} ({})", net.name, chain_id);
                                    println!("Fingerprint: {:?}", insp.fingerprint);
                                    println!(
                                        "ABI:         Unverified ({} candidate selectors)",
                                        insp.candidate_selectors.len()
                                    );
                                    println!("Selectors:   {}", hex_list.join(", "));
                                });
                            }
                            vaughan_core::browser::abi::AbiResolution::Error(err) => {
                                data["abi"] = json!({ "status": "error", "message": err });
                                json_out::print_json_value(json_mode, &data, || {
                                    println!("Address:     {}", addr.to_checksum(None));
                                    println!("Chain:       {} ({})", net.name, chain_id);
                                    println!("Fingerprint: {:?}", insp.fingerprint);
                                    println!("ABI:         Error ({err})");
                                });
                            }
                        }
                        Ok(())
                    }
                })
                .await?;
        }
        Command::Mcp { .. } => unreachable!("mcp handled in main"),
        Command::Serve { .. } => unreachable!("serve handled in main"),
        Command::Signerd { .. } => unreachable!("signerd handled in main"),
        Command::Preset { .. } => unreachable!("preset handled in main"),
        Command::Config { .. } => unreachable!("config handled in main"),
        Command::Propose { action } => {
            unlock(&mut wallet, None)?;
            let net = wallet.networks().active();
            let registry = default_assist_registry();
            let context = ToolContext {
                rpc_url: wallet.active_rpc_url(),
                chain_id: net.chain_id,
                active_address: wallet
                    .active_address()
                    .ok()
                    .and_then(|a| a.parse::<Address>().ok()),
                profile_dir: Some(vaughan_agent::paths::profile_dir(wallet.path())),
            };
            match action {
                ProposeCmd::Transfer {
                    recipient,
                    amount,
                    token,
                    explanation,
                } => {
                    let mut args = json!({
                        "recipient": recipient,
                        "amount": amount,
                        "explanation": explanation,
                    });
                    if let Some(t) = token {
                        args["token_address"] = json!(t);
                    }
                    let raw = registry
                        .execute("propose_transfer", args, &context)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let proposal: TxProposal = serde_json::from_value(raw)?;
                    guard_mainnet_write(net.is_testnet).map_err(|e| anyhow::anyhow!("{e}"))?;
                    let prof = profile_dir(wallet.path());
                    let secret = vaughan_core::core::McpSessionToken::read(&prof)?
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "MCP session token missing — unlock Vaughan TUI or run \
                                 `vaughan serve` on this profile first"
                            )
                        })?;
                    let queue = ProposalQueue::new(&prof);
                    let queued = queue
                        .enqueue(proposal.clone(), "cli", secret.as_bytes())
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let data = json!({
                        "proposal_id": queued.proposal.proposal_id,
                        "status": "pending_user",
                        "proposal": queued.proposal,
                    });
                    json_out::print_json_value(json_mode, &data, || {
                        println!("proposal_id: {}", queued.proposal.proposal_id);
                        println!("status: pending_user (open Vaughan TUI to approve)");
                    });
                }
                ProposeCmd::TokenLaunch {
                    name,
                    symbol,
                    supply,
                    explanation,
                } => {
                    let args = json!({
                        "name": name,
                        "symbol": symbol,
                        "supply": supply,
                        "explanation": explanation,
                    });
                    let raw = registry
                        .execute("propose_token_launch", args, &context)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let proposal: TxProposal = serde_json::from_value(raw)?;
                    guard_mainnet_write(net.is_testnet).map_err(|e| anyhow::anyhow!("{e}"))?;
                    let prof = profile_dir(wallet.path());
                    let secret = vaughan_core::core::McpSessionToken::read(&prof)?
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "MCP session token missing — unlock Vaughan TUI or run \
                                 `vaughan serve` on this profile first"
                            )
                        })?;
                    let queue = ProposalQueue::new(&prof);
                    let queued = queue
                        .enqueue(proposal.clone(), "cli", secret.as_bytes())
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let data = json!({
                        "proposal_id": queued.proposal.proposal_id,
                        "status": "pending_user",
                        "proposal": queued.proposal,
                    });
                    json_out::print_json_value(json_mode, &data, || {
                        println!("proposal_id: {}", queued.proposal.proposal_id);
                        println!("status: pending_user (open Vaughan TUI to approve token launch)");
                    });
                }
            }
        }
        Command::Proposals { action } => {
            let prof = profile_dir(wallet.path());
            let secret = vaughan_core::core::McpSessionToken::read(&prof)?.unwrap_or_default();
            let queue = ProposalQueue::new(&prof);
            match action {
                ProposalsCmd::List => {
                    let pending = queue.list_pending().map_err(|e| anyhow::anyhow!("{e}"))?;
                    let rows: Vec<_> = pending
                        .iter()
                        .map(|q| {
                            json!({
                                "proposal_id": q.proposal.proposal_id,
                                "source": q.source,
                                "chain_id": q.proposal.chain_id,
                            })
                        })
                        .collect();
                    let data = json!({ "pending": rows });
                    json_out::print_json_value(json_mode, &data, || {
                        if pending.is_empty() {
                            println!("no pending proposals");
                        }
                        for q in pending {
                            println!(
                                "{}  source={}  chain={}",
                                q.proposal.proposal_id, q.source, q.proposal.chain_id
                            );
                        }
                    });
                }
                ProposalsCmd::Show { proposal_id } => {
                    let queued = queue
                        .get_pending(&proposal_id, secret.as_bytes())
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let data = json!({
                        "proposal_id": proposal_id,
                        "status": "pending_user",
                        "source": queued.source,
                        "proposal": queued.proposal,
                    });
                    json_out::print_json_value(json_mode, &data, || {
                        if let Ok(text) = serde_json::to_string_pretty(&queued.proposal) {
                            println!("{text}");
                        }
                    });
                }
            }
        }
    }
    Ok(())
}

fn run_config(profile: String, json_mode: bool, action: ConfigCmd) -> anyhow::Result<()> {
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

fn run_preset(profile: String, json_mode: bool, action: PresetCmd) -> anyhow::Result<()> {
    match action {
        PresetCmd::List => {
            let ids = vaughan_agent::BUNDLED_PRESET_IDS;
            let data = json!({ "presets": ids, "root": vaughan_agent::presets_root().display().to_string() });
            json_out::print_json_value(json_mode, &data, || {
                println!("Bundled sentient presets:");
                for id in ids {
                    println!("  {id}");
                }
                println!(
                    "Apply: vaughan --profile sentient preset apply <id>\nDocs: docs/sentient-presets.md"
                );
            });
        }
        PresetCmd::Apply { id } => {
            let path = StateManager::profile_path(&profile).map_err(|e| anyhow::anyhow!("{e}"))?;
            let prof = profile_dir(&path);
            std::fs::create_dir_all(&prof)?;
            let skill_dir =
                vaughan_agent::apply_preset(&id, &prof).map_err(|e| anyhow::anyhow!("{e}"))?;
            let data = json!({
                "preset": id,
                "profile": profile,
                "skills_dir": skill_dir.display().to_string(),
                "policy": prof.join(vaughan_agent::SENTIENT_POLICY_TOML).display().to_string(),
            });
            json_out::print_json_value(json_mode, &data, || {
                println!("Applied preset `{id}` to profile `{profile}`");
                println!("  skills: {}", skill_dir.display());
                println!(
                    "  policy: {}",
                    prof.join(vaughan_agent::SENTIENT_POLICY_TOML).display()
                );
            });
        }
    }
    Ok(())
}

/// Unlock the vault, prompting for the password unless `--password-env` names
/// an env var holding it.
fn unlock(wallet: &mut WalletState, password_env: Option<&str>) -> anyhow::Result<()> {
    if !wallet.is_initialized() {
        anyhow::bail!(
            "no wallet found at {} — run `vaughan create` (or `vaughan restore`) first",
            wallet.path().display()
        );
    }
    let password = prompt_password(password_env)?;
    wallet.unlock(&password)?;
    Ok(())
}

fn prompt_password(password_env: Option<&str>) -> anyhow::Result<SecretString> {
    if let Some(var) = password_env {
        let value = std::env::var(var)
            .map_err(|_| anyhow::anyhow!("environment variable `{var}` is not set"))?;
        return Ok(SecretString::from(value));
    }
    prompt_secret("vault password: ")
}

/// Hidden interactive prompt; reads piped stdin verbatim when not a TTY so
/// scripts and tests never have to put secrets on argv.
fn prompt_secret(prompt: &str) -> anyhow::Result<SecretString> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return Ok(SecretString::from(rpassword::prompt_password(prompt)?));
    }
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(SecretString::from(buf.trim().to_string()))
}

fn confirm_sign() -> anyhow::Result<bool> {
    use std::io::Write;
    eprint!("sign this typed data? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(false);
    }
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn load_typed_data_json(
    data: Option<&str>,
    file: Option<&std::path::Path>,
) -> anyhow::Result<serde_json::Value> {
    let raw = match (data, file) {
        (Some(s), None) => s.to_string(),
        (None, Some(path)) => std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read typed-data file: {e}"))?,
        (None, None) => anyhow::bail!("provide --data or --file"),
        (Some(_), Some(_)) => unreachable!("clap conflicts_with"),
    };
    serde_json::from_str(raw.trim()).map_err(|e| anyhow::anyhow!("typed-data JSON: {e}"))
}

fn validate_typed_data_shape(v: &serde_json::Value) -> anyhow::Result<()> {
    if !v.is_object() {
        anyhow::bail!("typed-data must be a JSON object");
    }
    for key in ["types", "domain", "primaryType", "message"] {
        if v.get(key).is_none() {
            anyhow::bail!("typed-data missing `{key}`");
        }
    }
    Ok(())
}

fn gate_and_reserve_sentient_send(
    profile_dir: &std::path::Path,
    chain_id: u64,
    to_str: &str,
    val_str: &str,
    bal_raw: &str,
    data: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(d) = data {
        let clean = d.trim().strip_prefix("0x").unwrap_or(d.trim());
        if !clean.is_empty() {
            anyhow::bail!(
                "calldata_send_not_permitted: sentient profiles must use a typed proposal (propose_*) for contract calls"
            );
        }
    }
    let policy =
        vaughan_agent::sentient::load_policy(profile_dir).map_err(|e| anyhow::anyhow!("{e}"))?;
    let to_addr = Address::from_str(to_str)
        .map_err(|e| anyhow::anyhow!("invalid destination address `{to_str}`: {e}"))?;
    vaughan_agent::sentient::check_destination_allowed(&policy, to_addr)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let balance = U256::from_str(bal_raw)
        .or_else(|_| U256::from_str_radix(bal_raw.trim_start_matches("0x"), 16))
        .map_err(|e| anyhow::anyhow!("failed to parse wallet balance `{bal_raw}`: {e}"))?;
    let send_val = U256::from_str(val_str)
        .or_else(|_| U256::from_str_radix(val_str.trim_start_matches("0x"), 16))
        .map_err(|e| anyhow::anyhow!("failed to parse send value `{val_str}`: {e}"))?;
    let asset = format!("{chain_id}:native");
    let now = vaughan_agent::dca::now_unix();
    let data_bytes = data.unwrap_or("").as_bytes();
    let data_hash = alloy::primitives::keccak256(data_bytes);
    let key = format!("cli_send_{now}_{to_addr:#x}_{data_hash:#x}_{send_val}");
    vaughan_agent::sentient::SpendLedger::reserve(
        profile_dir,
        &key,
        &asset,
        send_val,
        balance,
        policy.daily_cap_pct,
        policy.enforcement,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(key)
}

/// `y/N` gate before broadcasting. Fails closed: EOF (piped/closed stdin)
/// and anything but an explicit "y"/"yes" mean "do not broadcast" — scripts
/// must pass `--yes`.
fn confirm_broadcast() -> anyhow::Result<bool> {
    use std::io::Write;
    eprint!("broadcast this transaction? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(false);
    }
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vaughan_core::core::parse_native_amount;

    #[test]
    fn send_parses_value_wei() {
        assert_eq!(parse_native_amount("0", 18).unwrap(), "0");
        assert_eq!(parse_native_amount("1", 18).unwrap(), "1000000000000000000");
    }

    #[test]
    fn build_contract_call_path_requires_no_wallet() {
        // The tx builder used by `send` validates calldata before any RPC.
        let svc = TransactionService::new();
        let tx = svc
            .build_contract_call("0xabc", "0xdef", "0x1234", "0", 943)
            .unwrap();
        match tx {
            ChainTransaction::Evm(e) => {
                assert_eq!(e.data.as_deref(), Some("0x1234"));
                assert_eq!(e.chain_id, 943);
            }
            _ => panic!("expected EVM variant"),
        }
    }

    #[test]
    fn sentient_cli_refuses_unparseable_value_or_destination() {
        let dir = tempfile::tempdir().unwrap();
        let policy = vaughan_agent::sentient::AgentSessionPolicy {
            allow_transfer_to: vec!["0x1111111111111111111111111111111111111111".into()],
            ..Default::default()
        };
        vaughan_agent::sentient::save_policy(dir.path(), &policy).unwrap();

        let valid_to = "0x1111111111111111111111111111111111111111";
        // 1. Unparseable value refuses
        let err1 =
            gate_and_reserve_sentient_send(dir.path(), 369, valid_to, "not_a_number", "1000", None);
        assert!(err1.is_err(), "expected error on unparseable value");
        assert!(err1
            .unwrap_err()
            .to_string()
            .contains("failed to parse send value"));

        // 2. Unparseable destination refuses
        let err2 =
            gate_and_reserve_sentient_send(dir.path(), 369, "not_an_address", "100", "1000", None);
        assert!(err2.is_err(), "expected error on unparseable destination");
        assert!(err2
            .unwrap_err()
            .to_string()
            .contains("invalid destination address"));

        // 3. Unparseable balance refuses
        let err3 =
            gate_and_reserve_sentient_send(dir.path(), 369, valid_to, "100", "not_a_balance", None);
        assert!(err3.is_err(), "expected error on unparseable balance");
        assert!(err3
            .unwrap_err()
            .to_string()
            .contains("failed to parse wallet balance"));
    }

    #[test]
    fn sentient_cli_refuses_non_empty_calldata() {
        let dir = tempfile::tempdir().unwrap();
        let valid_to = "0x1111111111111111111111111111111111111111";
        let policy = vaughan_agent::sentient::AgentSessionPolicy {
            allow_transfer_to: vec![valid_to.into()],
            ..Default::default()
        };
        vaughan_agent::sentient::save_policy(dir.path(), &policy).unwrap();

        // Non-empty calldata must be refused in sentient profiles
        let err = gate_and_reserve_sentient_send(
            dir.path(),
            369,
            valid_to,
            "0",
            "1000",
            Some("0xa9059cbb0000000000000000000000001111111111111111111111111111111111111111"),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.starts_with("calldata_send_not_permitted:"),
            "expected calldata_send_not_permitted, got: {msg}"
        );
    }

    #[test]
    fn sentient_profile_refuses_sign_typed_data_and_adviser_unaffected() {
        assert!(vaughan_core::core::is_sentient_profile("sentient"));
        assert!(vaughan_core::core::is_sentient_profile("degen"));
        assert!(!vaughan_core::core::is_sentient_profile("adviser"));
        assert!(!vaughan_core::core::is_sentient_profile("default"));
    }

    #[test]
    fn test_gate_and_reserve_sentient_send_returns_key_and_releases() {
        let dir = tempfile::tempdir().unwrap();
        let valid_to = "0x1111111111111111111111111111111111111111";
        let policy = vaughan_agent::sentient::AgentSessionPolicy {
            allow_transfer_to: vec![valid_to.into()],
            daily_cap_pct: 20,
            enforcement: vaughan_agent::sentient::EnforcementMode::Enforced,
            ..Default::default()
        };
        vaughan_agent::sentient::save_policy(dir.path(), &policy).unwrap();

        let key =
            gate_and_reserve_sentient_send(dir.path(), 369, valid_to, "100", "1000", None).unwrap();
        assert!(key.starts_with("cli_send_"));

        let snap = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(
            snap.assets.get("369:native").unwrap().spent,
            alloy::primitives::U256::from(100)
        );

        // When release_legs is called (e.g. send_transaction failed), spent is refunded
        vaughan_agent::sentient::release_legs(dir.path(), &key).unwrap();
        let snap2 = vaughan_agent::sentient::SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(
            snap2.assets.get("369:native").unwrap().spent,
            alloy::primitives::U256::ZERO
        );
    }
}
