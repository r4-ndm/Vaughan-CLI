//! Fixed-supply ERC-20 deploy for testnet meme-coin launches.
//!
//! Uses pinned creation bytecode from [`scripts/token-launch/FixedSupplyToken.sol`]
//! (compile via [`scripts/compile-token-launch.sh`]). Runtime path is pure Rust +
//! Alloy — no `forge` subprocess.

use alloy::primitives::{Address, U256};
use alloy::sol_types::SolConstructor;
use std::str::FromStr;
use std::time::{Duration, Instant};

use crate::chains::evm::EvmAdapter;
use crate::chains::EvmTransaction;
use crate::core::persistence::CustomToken;
use crate::core::transaction::{parse_native_amount, TransactionService};
use crate::error::WalletError;

/// Human-readable decimals for launched tokens (fixed in v1).
pub const TOKEN_LAUNCH_DECIMALS: u8 = 18;

/// Chains where the TUI token launcher is enabled (testnet-first).
pub fn token_launch_allowed(chain_id: u64) -> bool {
    matches!(chain_id, 943 | 31337)
}

/// Pinned creation bytecode (without constructor args).
const CREATION_BYTECODE: &str =
    include_str!("../../../scripts/token-launch/FixedSupplyToken.creation.hex");

alloy::sol! {
    #[sol(rpc)]
    contract FixedSupplyTokenDeploy {
        constructor(
            string name,
            string symbol,
            uint8 decimals,
            uint256 initialSupply,
            address recipient
        );
    }
}

/// Validate token name for deploy (1–32 chars after trim).
pub fn validate_token_name(name: &str) -> Result<String, WalletError> {
    let s = name.trim();
    if s.is_empty() || s.len() > 32 {
        return Err(WalletError::InvalidTransaction(
            "name must be 1–32 characters".into(),
        ));
    }
    Ok(s.to_string())
}

/// Validate ticker / symbol (1–11 alphanumeric, uppercased).
pub fn validate_token_symbol(symbol: &str) -> Result<String, WalletError> {
    let s = symbol.trim();
    if s.is_empty() || s.len() > 11 {
        return Err(WalletError::InvalidTransaction(
            "symbol must be 1–11 characters".into(),
        ));
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(WalletError::InvalidTransaction(
            "symbol must be letters and numbers only".into(),
        ));
    }
    Ok(s.to_ascii_uppercase())
}

/// Parse human supply string into raw base units (18 decimals).
pub fn parse_token_supply_human(supply: &str) -> Result<U256, WalletError> {
    let raw = parse_native_amount(supply.trim(), TOKEN_LAUNCH_DECIMALS)?;
    let amount =
        U256::from_str(&raw).map_err(|_| WalletError::InvalidAmount("invalid supply".into()))?;
    if amount.is_zero() {
        return Err(WalletError::InvalidAmount("supply must be > 0".into()));
    }
    Ok(amount)
}

fn decode_creation_bytecode() -> Result<Vec<u8>, WalletError> {
    let hex_str = CREATION_BYTECODE.trim();
    hex::decode(hex_str).map_err(|e| WalletError::InvalidTransaction(format!("bytecode: {e}")))
}

/// ABI-encode contract-creation calldata for a fixed-supply token deploy.
pub fn encode_erc20_deploy_calldata(
    name: &str,
    symbol: &str,
    supply_raw: U256,
    recipient: Address,
) -> Result<Vec<u8>, WalletError> {
    let name = validate_token_name(name)?;
    let symbol = validate_token_symbol(symbol)?;
    let init = decode_creation_bytecode()?;
    let args = FixedSupplyTokenDeploy::constructorCall {
        name,
        symbol,
        decimals: TOKEN_LAUNCH_DECIMALS,
        initialSupply: supply_raw,
        recipient,
    }
    .abi_encode();
    let mut out = init;
    out.extend_from_slice(&args);
    Ok(out)
}

/// Build an unsigned contract-creation [`EvmTransaction`].
pub fn build_erc20_deploy_evm(
    from: &str,
    chain_id: u64,
    name: &str,
    symbol: &str,
    supply_human: &str,
    recipient: Address,
) -> Result<EvmTransaction, WalletError> {
    if !token_launch_allowed(chain_id) {
        return Err(WalletError::InvalidTransaction(format!(
            "token launch is testnet-only today (chain {chain_id})"
        )));
    }
    let supply_raw = parse_token_supply_human(supply_human)?;
    let data = encode_erc20_deploy_calldata(name, symbol, supply_raw, recipient)?;
    TransactionService::new()
        .build_contract_call(
            from,
            "0x0000000000000000000000000000000000000000",
            &format!("0x{}", hex::encode(&data)),
            "0",
            chain_id,
        )
        .and_then(|tx| match tx {
            crate::chains::ChainTransaction::Evm(evm) => Ok(evm),
            _ => Err(WalletError::InvalidTransaction(
                "expected EVM transaction".into(),
            )),
        })
}

/// Poll until `eth_getTransactionReceipt` returns a `contractAddress`.
pub async fn wait_for_deployed_address(
    adapter: &EvmAdapter,
    tx_hash: &str,
    timeout: Duration,
) -> Result<Address, WalletError> {
    use alloy::primitives::B256;
    use alloy::providers::Provider;

    let h = tx_hash.trim_start_matches("0x");
    let bytes =
        hex::decode(h).map_err(|_| WalletError::InvalidTransaction("invalid tx hash".into()))?;
    if bytes.len() != 32 {
        return Err(WalletError::InvalidTransaction(
            "tx hash must be 32 bytes".into(),
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    let hash = B256::from(arr);

    let deadline = Instant::now() + timeout;
    loop {
        let receipt = adapter
            .with_provider(|provider| async move {
                provider
                    .get_transaction_receipt(hash)
                    .await
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            })
            .await?;
        if let Some(r) = receipt {
            if let Some(addr) = r.contract_address {
                return Ok(addr);
            }
            return Err(WalletError::InvalidTransaction(
                "deploy tx receipt missing contractAddress".into(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(WalletError::NetworkError(
                "timed out waiting for deploy receipt".into(),
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Outcome of a successful token deploy + import.
#[derive(Debug, Clone)]
pub struct TokenLaunchOutcome {
    pub tx_hash: String,
    pub contract: Address,
    pub token: CustomToken,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn validate_name_and_symbol() {
        assert!(validate_token_name("Pepe Jr").is_ok());
        assert!(validate_token_symbol("pepe2").unwrap() == "PEPE2");
        assert!(validate_token_symbol("bad ticker").is_err());
    }

    #[test]
    fn encode_deploy_calldata_non_empty() {
        let data = encode_erc20_deploy_calldata(
            "Test Coin",
            "TEST",
            U256::from(1_000_000u64) * U256::from(10).pow(U256::from(18)),
            address!("0x0000000000000000000000000000000000000001"),
        )
        .expect("encode");
        assert!(data.len() > 100);
        assert!(decode_creation_bytecode().unwrap().len() < data.len());
    }

    #[test]
    fn build_deploy_tx_uses_create_target() {
        let tx = build_erc20_deploy_evm(
            "0x0000000000000000000000000000000000000001",
            943,
            "Meme",
            "MEME",
            "1000000",
            address!("0x0000000000000000000000000000000000000001"),
        )
        .unwrap();
        assert_eq!(tx.to, "0x0000000000000000000000000000000000000000");
        assert!(tx.data.as_ref().is_some_and(|d| d.len() > 10));
    }

    #[test]
    fn mainnet_chain_rejected() {
        assert!(!token_launch_allowed(369));
        assert!(build_erc20_deploy_evm("0x1", 369, "X", "X", "1", Address::ZERO,).is_err());
    }
}
