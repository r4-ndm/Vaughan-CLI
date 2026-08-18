//! Broadcast a signed [`crate::scw::ScwTransaction`] through the EIP-7702
//! self-pay path.
//!
//! [`crate::build`] assembles the [`TxEip7702`] carrying `execute(txns,
//! signature)`; this module submits it. The account EOA self-pays gas
//! (testnet-first, NFR-3): we fetch the account's *pending* nonce, derive
//! EIP-1559 fees through the adapter's existing heuristic, sign the 7702
//! envelope with the account key, and hand the raw signed transaction to the
//! adapter's primary + fallback broadcast path.
//!
//! The relayer / ERC-4337-bundler routes are intentionally not implemented
//! here — see TASKS.md (FR-3.3) and `docs/ambire-aa.md`.

use alloy::consensus::SignableTransaction;
use alloy::eips::eip2718::Encodable2718;
use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;

use vaughan_core::chains::evm::EvmAdapter;
use vaughan_core::chains::{ChainAdapter, ChainTransaction, EvmTransaction, FeeDetails, TxHash};
use vaughan_core::error::WalletError;

use crate::build::{build_7702_transaction, sign_authorization};
use crate::encode::encode_execute;
use crate::scw::ScwTransaction;

/// Default gas limit for the 7702 execute call when the caller has no tighter
/// estimate.
///
/// `eth_estimateGas` cannot price an EIP-7702 call before the delegation
/// exists on-chain (the RPC sees a bare EOA), so the first submission uses
/// this conservative constant. Unused gas is refunded, so over-estimating
/// only wastes a fee *buffer*, never actual spend; once the account is
/// delegated, callers can pass a tighter limit.
pub const DEFAULT_BATCH_GAS_LIMIT: u64 = 1_000_000;

/// Build the `ChainTransaction::Evm` mirroring the 7702 execute call, used to
/// derive EIP-1559 fees through the adapter's existing heuristic.
///
/// `from == to == tx.account` mirrors the real transaction (which calls the
/// now-delegated EOA with the batch payload). `gas_limit` is pinned so the
/// fee estimate skips RPC gas estimation (which would be wrong pre-delegation)
/// and only prices the base fee.
fn fee_mirror(tx: &ScwTransaction, signature: &[u8], gas_limit: u64) -> ChainTransaction {
    let calldata = encode_execute(&tx.txns, signature);
    ChainTransaction::Evm(EvmTransaction {
        from: tx.account.to_string(),
        to: tx.account.to_string(),
        value: "0".into(),
        data: Some(format!("0x{}", hex::encode(calldata))),
        gas_limit: Some(gas_limit),
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id: tx.chain_id,
    })
}

/// Estimate the fee for the 7702 execute call via `adapter`'s EIP-1559
/// heuristic. Returns `(gas_limit, max_fee_per_gas, max_priority_fee_per_gas)`.
///
/// `gas_limit` defaults to [`DEFAULT_BATCH_GAS_LIMIT`] when `None`.
pub async fn estimate_self_pay_fee(
    adapter: &dyn ChainAdapter,
    tx: &ScwTransaction,
    signature: &[u8],
    gas_limit: Option<u64>,
) -> Result<(u64, u128, u128), WalletError> {
    let gas_limit = gas_limit.unwrap_or(DEFAULT_BATCH_GAS_LIMIT);
    let fee = adapter
        .estimate_fee(&fee_mirror(tx, signature, gas_limit))
        .await?;
    let FeeDetails::Evm {
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas,
    } = fee.details
    else {
        return Err(WalletError::InvalidTransaction(
            "expected EVM fee details".to_string(),
        ));
    };
    let max_fee = max_fee_per_gas
        .as_deref()
        .ok_or_else(|| WalletError::InvalidTransaction("no max fee in estimate".to_string()))?
        .parse::<u128>()
        .map_err(|_| WalletError::InvalidTransaction("bad max fee in estimate".to_string()))?;
    // Priority is absent on legacy-style responses; EIP-7702 requires the
    // field, and zero is a valid tip on EIP-1559 chains (the field is only
    // ever absent pre-London, which 7702 cannot run on anyway).
    let priority = match max_priority_fee_per_gas.as_deref() {
        Some(p) => p
            .parse::<u128>()
            .map_err(|_| WalletError::InvalidTransaction("bad priority fee".to_string()))?,
        None => 0,
    };
    Ok((gas_limit, max_fee, priority))
}

/// Sign the EIP-7702 envelope that submits `tx`'s batch, returning the raw
/// EIP-2718-encoded transaction (no leading `0x`).
///
/// The authorization is signed with `account_nonce + 1`, *not* the account
/// nonce: EIP-7702 processes the authorization list after the sender's nonce
/// has been incremented, and the authority's nonce is validated against that
/// post-increment state. The outer transaction itself uses `account_nonce`.
/// This is the well-known self-pay 7702 nonce rule — see
/// <https://eips.ethereum.org/EIPS/eip-7702> ("after the sender's nonce is
/// incremented").
///
/// `signer` must be the key that owns `tx.account` (the account EOA): it signs
/// the batch, the authorization, and the outer envelope.
#[allow(clippy::too_many_arguments)]
pub fn sign_7702_envelope(
    signer: &PrivateKeySigner,
    tx: &ScwTransaction,
    signature: &[u8],
    implementation: Address,
    account_nonce: u64,
    gas_limit: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
) -> Result<Vec<u8>, WalletError> {
    if signer.address() != tx.account {
        return Err(WalletError::InvalidTransaction(
            "batch signer must own the account EOA".to_string(),
        ));
    }
    let authorization = sign_authorization(signer, tx.chain_id, implementation, account_nonce + 1)?;
    let built = build_7702_transaction(
        tx,
        signature,
        authorization,
        account_nonce,
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas,
    )?;
    let envelope_signature = signer
        .sign_hash_sync(&built.signature_hash())
        .map_err(|e| WalletError::SigningFailed(e.to_string()))?;
    Ok(built.into_signed(envelope_signature).encoded_2718())
}

/// Submit `tx`'s signed batch through the EIP-7702 self-pay path: fetch the
/// account's pending nonce and a fee estimate, sign the envelope, broadcast.
///
/// `signature` is the 66-byte `r ‖ s ‖ v ‖ mode` batch signature produced by
/// [`crate::sign::sign_scw_transaction`]; `implementation` is the deployed
/// `AmbireAccount` implementation the account delegates to.
///
/// Assumes the account has no other in-flight transactions: the authorization
/// nonce (`pending + 1`) is only correct when this 7702 tx is the account's
/// next transaction. Testnet-first (NFR-3).
pub async fn submit_self_pay(
    adapter: &EvmAdapter,
    signer: &PrivateKeySigner,
    tx: &ScwTransaction,
    signature: &[u8],
    implementation: Address,
    gas_limit: Option<u64>,
) -> Result<TxHash, WalletError> {
    if signer.address() != tx.account {
        return Err(WalletError::InvalidTransaction(
            "batch signer must own the account EOA".to_string(),
        ));
    }
    let account_nonce = adapter.get_pending_nonce(&tx.account.to_string()).await?;
    let (gas_limit, max_fee, priority) =
        estimate_self_pay_fee(adapter, tx, signature, gas_limit).await?;
    let raw = sign_7702_envelope(
        signer,
        tx,
        signature,
        implementation,
        account_nonce,
        gas_limit,
        max_fee,
        priority,
    )?;
    adapter.broadcast_raw(raw).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::consensus::{transaction::RlpEcdsaDecodableTx, TxEip7702};
    use alloy::primitives::{Bytes, U256};
    use vaughan_core::security::hd_wallet::{derive_account, validate_mnemonic};

    use crate::abi::Transaction;
    use crate::scw::SignatureMode;
    use crate::sign::sign_scw_transaction;

    // Canonical BIP-39/BIP-44 test vector (all-"abandon" mnemonic).
    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn signer() -> PrivateKeySigner {
        derive_account(&validate_mnemonic(TEST_MNEMONIC).unwrap(), 0).unwrap()
    }

    fn sample_tx() -> ScwTransaction {
        ScwTransaction {
            account: signer().address(),
            chain_id: 943,
            nonce: 3,
            txns: vec![Transaction {
                to: Address::from([0x22u8; 20]),
                value: U256::from(42u64),
                data: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            }],
        }
    }

    #[test]
    fn fee_mirror_pins_gas_and_payload() {
        let tx = sample_tx();
        let sig = sign_scw_transaction(&signer(), &tx, SignatureMode::RawHash).unwrap();
        let mirror = fee_mirror(&tx, &sig, 250_000);

        let ChainTransaction::Evm(evm) = mirror else {
            panic!("expected EVM mirror");
        };
        assert_eq!(evm.from, tx.account.to_string());
        assert_eq!(evm.to, tx.account.to_string());
        assert_eq!(evm.value, "0");
        assert_eq!(evm.gas_limit, Some(250_000));
        assert_eq!(evm.chain_id, 943);
        // The mirror's data must be the exact execute calldata that will ship.
        let expected = format!("0x{}", hex::encode(encode_execute(&tx.txns, &sig)));
        assert_eq!(evm.data.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn envelope_is_type_4_and_recovers_to_account() {
        let signer = signer();
        let tx = sample_tx();
        let implementation = Address::from([0xaau8; 20]);
        let sig = sign_scw_transaction(&signer, &tx, SignatureMode::RawHash).unwrap();

        let raw = sign_7702_envelope(
            &signer,
            &tx,
            &sig,
            implementation,
            7,
            100_000,
            2_000_000_000,
            1_000_000_000,
        )
        .unwrap();

        // EIP-2718 type byte for the set-code transaction is 0x04.
        assert_eq!(raw[0], 0x04);
        let signed = TxEip7702::rlp_decode_signed(&mut &raw[1..]).unwrap();
        assert_eq!(signed.tx().chain_id, 943);
        assert_eq!(signed.tx().nonce, 7);
        assert_eq!(signed.tx().to, tx.account);
        // The authorization inside must be for the *next* nonce (self-pay rule)
        // and delegate to the implementation.
        let auth = &signed.tx().authorization_list[0];
        assert_eq!(
            auth.inner().nonce(),
            8,
            "auth nonce must be account nonce + 1"
        );
        assert_eq!(auth.inner().address(), &implementation);
        // The envelope signature must recover to the account key.
        let recovered = signed
            .signature()
            .recover_address_from_prehash(&signed.tx().signature_hash())
            .unwrap();
        assert_eq!(recovered, signer.address());
    }

    #[test]
    fn envelope_rejects_non_owner_signer() {
        let signer = signer();
        let other = derive_account(&validate_mnemonic(TEST_MNEMONIC).unwrap(), 1).unwrap();
        let tx = sample_tx();
        let sig = sign_scw_transaction(&signer, &tx, SignatureMode::RawHash).unwrap();

        let err = sign_7702_envelope(
            &other,
            &tx,
            &sig,
            Address::from([0xaau8; 20]),
            7,
            100_000,
            2_000_000_000,
            1_000_000_000,
        )
        .unwrap_err();
        assert!(matches!(err, WalletError::InvalidTransaction(_)));
    }
}
