//! EVM hardware / local signing helpers (profile layer).
//!
//! Builds Alloy transaction envelopes from prepared [`EvmTransaction`] fields.
//! Device I/O stays in [`super::super::DeviceSession`]; this module only knows EVM.

use std::str::FromStr;

use alloy::eips::eip2718::Encodable2718;
use alloy::network::{EthereumWallet, NetworkTransactionBuilder};
use alloy::primitives::{Address, TxKind, U256};
use alloy::rpc::types::eth::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;

use crate::chains::EvmTransaction;
use crate::error::WalletError;
use crate::security::signing::{sign_hash, sign_personal_message, sign_typed_data};

use super::super::types::{SignRequest, SignResult};

/// Default BIP-44 Ethereum path for account index `i` (`m/44'/60'/0'/0/{i}`).
pub fn default_evm_derivation_path(account_index: u32) -> String {
    format!("m/44'/60'/0'/0/{account_index}")
}

/// Sign an EVM [`SignRequest`] with a local [`PrivateKeySigner`].
pub async fn sign_evm_local(
    signer: &PrivateKeySigner,
    req: SignRequest,
) -> Result<SignResult, WalletError> {
    match req {
        SignRequest::EvmPersonal { message } => Ok(SignResult::SignatureHex(
            sign_personal_message(signer, &message)?,
        )),
        SignRequest::EvmTypedData { payload } => {
            Ok(SignResult::SignatureHex(sign_typed_data(signer, &payload)?))
        }
        SignRequest::EvmTypedDataHash { hash } => {
            Ok(SignResult::SignatureHex(sign_hash(signer, &hash)?))
        }
        SignRequest::EvmTransaction { tx } => {
            let raw = sign_prepared_evm_tx(signer, &tx).await?;
            Ok(SignResult::RawTx(raw))
        }
    }
}

/// Sign EIP-712 JSON via local signer.
pub fn sign_evm_typed_data_local(
    signer: &PrivateKeySigner,
    typed_data: &serde_json::Value,
) -> Result<String, WalletError> {
    sign_typed_data(signer, typed_data)
}

/// Sign a fully prepared EVM tx (nonce/gas/fees set) into an EIP-2718 envelope.
///
/// Does not query RPC. Callers fill nonce via [`crate::chains::evm::EvmAdapter`].
pub async fn sign_prepared_evm_tx(
    signer: &PrivateKeySigner,
    evm_tx: &EvmTransaction,
) -> Result<Vec<u8>, WalletError> {
    if evm_tx.nonce.is_none() {
        return Err(WalletError::InvalidTransaction(
            "nonce required before hardware/local envelope sign".into(),
        ));
    }
    let from = parse_addr(&evm_tx.from)?;
    if from != signer.address() {
        return Err(WalletError::SigningFailed(
            "transaction from does not match signer".into(),
        ));
    }
    let to = parse_addr(&evm_tx.to)?;
    let value = U256::from_str(&evm_tx.value)
        .map_err(|_| WalletError::InvalidAmount(format!("Invalid wei value: {}", evm_tx.value)))?;
    let is_create = to.is_zero()
        && evm_tx
            .data
            .as_deref()
            .is_some_and(|d| !d.trim_start_matches("0x").is_empty());
    let mut req = TransactionRequest {
        from: Some(from),
        to: Some(if is_create {
            TxKind::Create
        } else {
            TxKind::Call(to)
        }),
        value: Some(value),
        chain_id: Some(evm_tx.chain_id),
        nonce: evm_tx.nonce,
        gas: evm_tx.gas_limit,
        ..Default::default()
    };
    if evm_tx.max_fee_per_gas.is_none() {
        if let Some(gas_price) = evm_tx.gas_price.as_deref() {
            let gp = U256::from_str(gas_price).map_err(|_| {
                WalletError::InvalidAmount(format!("Invalid gas price: {gas_price}"))
            })?;
            req.gas_price = Some(gp.to::<u128>());
        }
    }
    if let Some(max_fee) = evm_tx.max_fee_per_gas.as_deref() {
        let mf = U256::from_str(max_fee)
            .map_err(|_| WalletError::InvalidAmount(format!("Invalid max fee: {max_fee}")))?;
        req.max_fee_per_gas = Some(mf.to::<u128>());
    }
    if let Some(prio) = evm_tx.max_priority_fee_per_gas.as_deref() {
        let p = U256::from_str(prio)
            .map_err(|_| WalletError::InvalidAmount(format!("Invalid priority fee: {prio}")))?;
        req.max_priority_fee_per_gas = Some(p.to::<u128>());
    }
    if let Some(data_hex) = evm_tx.data.as_deref() {
        let input_bytes = hex::decode(data_hex.trim_start_matches("0x"))
            .map_err(|_| WalletError::InvalidTransaction("Invalid hex data".to_string()))?;
        req.input.input = Some(input_bytes.into());
    }

    // Local path: Alloy EthereumWallet (MetaMask-family EIP-1559 envelope).
    // Hardware Phase 1 will sign the same prepared fields via DeviceSession.
    let wallet = EthereumWallet::from(signer.clone());
    let envelope = req
        .build(&wallet)
        .await
        .map_err(|e| WalletError::SigningFailed(e.to_string()))?;
    Ok(envelope.encoded_2718())
}

fn parse_addr(s: &str) -> Result<Address, WalletError> {
    Address::from_str(s.trim())
        .map_err(|_| WalletError::InvalidTransaction(format!("invalid address: {s}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::hd_wallet::{derive_account, validate_mnemonic};

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn default_path_matches_bip44_eth() {
        assert_eq!(default_evm_derivation_path(0), "m/44'/60'/0'/0/0");
        assert_eq!(default_evm_derivation_path(3), "m/44'/60'/0'/0/3");
    }

    #[tokio::test]
    async fn local_personal_sign_via_profile() {
        let signer = derive_account(&validate_mnemonic(TEST_MNEMONIC).unwrap(), 0).unwrap();
        let res = sign_evm_local(
            &signer,
            SignRequest::EvmPersonal {
                message: b"hello".to_vec(),
            },
        )
        .await
        .unwrap();
        let SignResult::SignatureHex(hex) = res else {
            panic!("expected signature");
        };
        assert!(hex.starts_with("0x"));
        assert_eq!(hex::decode(&hex[2..]).unwrap().len(), 65);
    }
}
