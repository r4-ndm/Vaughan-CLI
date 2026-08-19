//! vaughan — non-interactive wallet commands.
//!
//! Scriptable access to the Vaughan vault: broadcast contract calls (the path
//! used for testnet contract deploys), check balances, and manage the wallet.
//! All commands that touch the vault require the wallet password — via
//! `--password-env NAME` for automation, or an interactive prompt.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use secrecy::SecretString;
use vaughan_core::chains::ChainTransaction;
use vaughan_core::core::{OperatingMode, StateManager, TransactionService, WalletState};

#[derive(Debug, Parser)]
#[command(name = "vaughan", version, about = "Vaughan wallet CLI")]
struct Cli {
    /// Vault file path (default: the profile data dir).
    #[arg(long, global = true)]
    vault: Option<PathBuf>,

    /// Profile name (default: "default"; isolated degen bot: "degen").
    #[arg(long, global = true, default_value = "default")]
    profile: String,

    /// Operating mode (human, assist, degen).
    #[arg(long, global = true)]
    mode: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let profile = cli.profile;
    let mode = match cli.mode.as_deref() {
        Some("degen") | Some("degen-trader") => OperatingMode::DegenTrader,
        Some("assist") | Some("ai-assisted") => OperatingMode::AiAssisted,
        Some("human") | Some("human-only") | None => OperatingMode::HumanOnly,
        Some(unknown) => {
            anyhow::bail!("unknown operating mode: '{unknown}'. Valid: human, assist, degen")
        }
    };
    let path = if let Some(custom_vault) = cli.vault {
        custom_vault
    } else {
        StateManager::profile_path(&profile).unwrap_or_else(|_| {
            eprintln!("error: could not resolve the profile vault path; pass --vault");
            std::process::exit(1);
        })
    };
    let mut wallet = WalletState::load_with_session(path, mode, profile)?;

    match cli.command {
        Command::Networks => {
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
            println!(
                "{}  ({} {})",
                wallet.active_address()?,
                balance.formatted,
                balance.token.symbol
            );
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
                            println!("0x{}", hex::encode(&out));
                            return Ok(());
                        }

                        let insp = eng.inspect(&provider, chain_id, addr).await;
                        println!("Address:     {}", addr.to_checksum(None));
                        println!("Chain:       {} ({})", net.name, chain_id);
                        println!("Fingerprint: {:?}", insp.fingerprint);

                        match &insp.abi_resolution {
                            vaughan_core::browser::abi::AbiResolution::Verified(abi) => {
                                println!(
                                    "ABI:         Verified ({} functions)",
                                    abi.functions.len()
                                );
                                if let Some(fn_name) = c {
                                    let res = eng
                                        .call_named(&provider, addr, abi, &fn_name, &av)
                                        .await
                                        .map_err(vaughan_core::error::WalletError::RpcError)?;
                                    if res.decoded_values.is_empty() {
                                        println!("Result:      0x{}", hex::encode(&res.raw_output));
                                    } else {
                                        println!("Result:      {}", res.decoded_values.join(", "));
                                    }
                                } else {
                                    let mut names: Vec<_> =
                                        abi.functions.keys().map(|k| k.as_str()).collect();
                                    names.sort_unstable();
                                    println!("Functions:   {}", names.join(", "));
                                }
                            }
                            vaughan_core::browser::abi::AbiResolution::Unverified => {
                                println!(
                                    "ABI:         Unverified ({} candidate selectors)",
                                    insp.candidate_selectors.len()
                                );
                                let hex_list: Vec<_> = insp
                                    .candidate_selectors
                                    .iter()
                                    .map(|s| vaughan_core::browser::selectors::selector_to_hex(*s))
                                    .collect();
                                println!("Selectors:   {}", hex_list.join(", "));
                            }
                            vaughan_core::browser::abi::AbiResolution::Error(err) => {
                                println!("ABI:         Error ({err})");
                            }
                        }
                        Ok(())
                    }
                })
                .await?;
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
