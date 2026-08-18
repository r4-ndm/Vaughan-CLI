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
use vaughan_core::core::{StateManager, TransactionService, WalletState};

#[derive(Debug, Parser)]
#[command(name = "vaughan", version, about = "Vaughan wallet CLI")]
struct Cli {
    /// Vault file path (default: the standard data dir).
    #[arg(long, global = true)]
    vault: Option<PathBuf>,

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
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let path = cli.vault.unwrap_or_else(|| StateManager::default_path().unwrap_or_else(|_| {
        eprintln!("error: could not resolve the default vault path; pass --vault");
        std::process::exit(1);
    }));
    let mut wallet = WalletState::load(path)?;

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
        Command::Restore { phrase, password_env } => {
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
