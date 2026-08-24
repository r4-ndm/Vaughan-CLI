//! vaughan — Vaughan wallet: interactive TUI by default, scriptable subcommands.
//!
//! Run `vaughan` with no arguments to open the terminal wallet. Subcommands
//! (`send`, `balance`, `browse`, …) provide non-interactive vault access for
//! scripts and CI. All vault-touching subcommands require the wallet password
//! — via `--password-env NAME` for automation, or an interactive prompt.

mod json_out;

use std::path::PathBuf;

use alloy::primitives::Address;
use clap::{Parser, Subcommand};
use secrecy::SecretString;
use serde_json::json;
use vaughan_agent::paths::profile_dir;
use vaughan_agent::tools::{default_assist_registry, ToolContext};
use vaughan_core::chains::ChainTransaction;
use vaughan_core::core::proposal::{ProposalQueue, TxProposal};
use vaughan_core::core::{OperatingMode, StateManager, TransactionService, WalletState};

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
    /// Restore a wallet from a mnemonic phrase.
    Restore {
        /// The 12/24-word mnemonic.
        phrase: String,
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
        /// Client label shown on approval cards (e.g. cursor, claude).
        #[arg(long, default_value = "cursor")]
        source: String,
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
}

#[derive(Debug, Subcommand)]
enum ProposalsCmd {
    /// List pending proposals.
    List,
    /// Show one proposal by id.
    Show { proposal_id: String },
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
            vaughan_tui::run_interactive().map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(Command::Mcp { source }) => {
            let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            runtime.block_on(vaughan_mcp::run_stdio_server(profile, source))
                .map_err(|e| anyhow::anyhow!("{e}"))
        }
        Some(command) => {
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
            let nets: Vec<_> = wallet
                .networks()
                .networks()
                .iter()
                .map(|net| {
                    json!({
                        "id": net.id,
                        "chain_id": net.chain_id,
                        "testnet": net.is_testnet,
                        "rpc_url": net.rpc_url,
                    })
                })
                .collect();
            let data = json!({
                "networks": nets,
                "active": wallet.networks().active_id(),
            });
            json_out::print_json_value(json_mode, &data, || {
                for net in wallet.networks().networks() {
                    println!(
                        "{:<24} chain {}  {}  {}",
                        net.id,
                        net.chain_id,
                        if net.is_testnet { "testnet" } else { "mainnet" },
                        net.rpc_url
                    );
                }
                println!("active: {}", wallet.networks().active_id());
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
            println!("\n⚠️  mnemonic (write this down, it is shown only once):");
            println!("{mnemonic}");
        }
        Command::Restore {
            phrase,
            password_env,
        } => {
            if wallet.is_initialized() {
                anyhow::bail!("a wallet already exists at {}", wallet.path().display());
            }
            let password = prompt_password(password_env.as_deref())?;
            wallet.restore(&password, &phrase)?;
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
            // The CLI prints the request; explicit user consent was the
            // decision to run the command.
            eprintln!(
                "network: {} (chain {})\nfrom:    {}\nto:      {}\nvalue:   {} wei{}",
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
            let hash = wallet.send_transaction(evm).await?;
            println!("{hash}");
        }
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
                                            println!("Result:      0x{}", hex::encode(&res.raw_output));
                                        } else {
                                            println!("Result:      {}", res.decoded_values.join(", "));
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
                    let prof = profile_dir(wallet.path());
                    let secret = vaughan_core::core::McpSessionToken::read(&prof)?
                        .unwrap_or_default();
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
            }
        }
        Command::Proposals { action } => {
            let prof = profile_dir(wallet.path());
            let secret = vaughan_core::core::McpSessionToken::read(&prof)?
                .unwrap_or_default();
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
    let value = rpassword::prompt_password("vault password: ")?;
    Ok(SecretString::from(value))
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
}
