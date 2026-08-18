//! EIP-7702 transaction assembly: wrap a signed smart-account batch into a
//! transaction an EOA can submit on its own behalf.
//!
//! With [EIP-7702] the account EOA signs an *authorization* delegating its own
//! code to the Ambire `AmbireAccount` implementation, then submits an ordinary
//! transaction to itself carrying `execute(txns, signature)`. Inside that call
//! `address(this)` is the EOA, which is exactly the `account` field the digest
//! binds the batch to. No bundler or relayer is involved: the EOA self-pays gas
//! (testnet-first, NFR-3).
//!
//! The ERC-4337 `UserOperation` path (bundler submission) is intentionally
//! *not* assembled here yet — it needs its own EntryPoint/bundler decision (see
//! TASKS.md FR-3.3 and `docs/ambire-aa.md`).
//!
//! [EIP-7702]: https://eips.ethereum.org/EIPS/eip-7702

use alloy::consensus::TxEip7702;
use alloy::eips::eip7702::{Authorization, SignedAuthorization};
use alloy::primitives::{Address, Bytes, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;

use vaughan_core::error::WalletError;

use crate::encode::encode_execute;
use crate::scw::ScwTransaction;

/// Sign the EIP-7702 authorization that delegates the account EOA's code to
/// `implementation`, bound to `chain_id` and replay-protected by `nonce`.
///
/// The authority is [`PrivateKeySigner`]'s address; callers must pass the same
/// key that owns `ScwTransaction::account`, or the resulting transaction will
/// fail account-side validation.
pub fn sign_authorization(
    signer: &PrivateKeySigner,
    chain_id: u64,
    implementation: Address,
    nonce: u64,
) -> Result<SignedAuthorization, WalletError> {
    let authorization = Authorization {
        chain_id: U256::from(chain_id),
        address: implementation,
        nonce,
    };
    let signature = signer
        .sign_hash_sync(&authorization.signature_hash())
        .map_err(|e| WalletError::SigningFailed(e.to_string()))?;
    Ok(authorization.into_signed(signature))
}

/// Assemble the EIP-7702 transaction that submits `tx`'s batch.
///
/// The transaction calls the (now-delegated) account EOA with
/// `execute(txns, signature)`, so the smart account sees `address(this) ==
/// tx.account` and validates the 66-byte `r ‖ s ‖ v ‖ mode` signature against
/// the same digest. `authorization` must be the signed delegation produced by
/// [`sign_authorization`] for the *same* account; it is recovered and checked
/// against `tx.account` before anything is assembled.
///
/// The caller (broadcast layer) supplies the transaction-level `nonce`, gas and
/// fee fields, which are network state and stay out of this pure builder.
#[allow(clippy::too_many_arguments)]
pub fn build_7702_transaction(
    tx: &ScwTransaction,
    signature: &[u8],
    authorization: SignedAuthorization,
    nonce: u64,
    gas_limit: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
) -> Result<TxEip7702, WalletError> {
    if signature.len() != 66 {
        return Err(WalletError::InvalidTransaction(format!(
            "expected a 66-byte r‖s‖v‖mode signature, got {} bytes",
            signature.len()
        )));
    }
    // Validate before assembling: the delegation must be authorized by the
    // very account the batch is bound to, and bound to the same chain. Recover
    // the authority from the signature over the spec's signature_hash preimage
    // (avoids the `k256` feature that `SignedAuthorization::recover_authority`
    // is gated behind).
    let authority = authorization
        .signature()
        .and_then(|sig| sig.recover_address_from_prehash(&authorization.inner().signature_hash()))
        .map_err(|e| {
            WalletError::InvalidTransaction(format!("authorization recovery failed: {e}"))
        })?;
    if authority != tx.account {
        return Err(WalletError::InvalidTransaction(format!(
            "authorization authority {authority} does not match account {}",
            tx.account
        )));
    }
    if authorization.inner().chain_id() != &U256::from(tx.chain_id) {
        return Err(WalletError::InvalidTransaction(format!(
            "authorization chain id {} does not match batch chain id {}",
            authorization.inner().chain_id(),
            tx.chain_id
        )));
    }

    let call_data: Bytes = encode_execute(&tx.txns, signature).into();
    Ok(TxEip7702 {
        chain_id: tx.chain_id,
        nonce,
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        to: tx.account,
        value: U256::ZERO,
        access_list: Default::default(),
        authorization_list: vec![authorization],
        input: call_data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;
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
    fn authorization_recovers_to_the_account_key() {
        let signer = signer();
        let implementation = Address::from([0xaau8; 20]);
        let auth = sign_authorization(&signer, 943, implementation, 7).unwrap();

        // The signed authorization must recover to the account EOA, and the
        // recovered signature must cover the spec's signature_hash preimage.
        let hash = auth.inner().signature_hash();
        let recovered = auth
            .signature()
            .unwrap()
            .recover_address_from_prehash(&hash)
            .unwrap();
        assert_eq!(recovered, signer.address());
        assert_eq!(auth.inner().address(), &implementation);
        assert_eq!(auth.inner().chain_id(), &U256::from(943u64));
        assert_eq!(auth.inner().nonce(), 7);
    }

    #[test]
    fn builds_7702_tx_with_execute_calldata() {
        let signer = signer();
        let tx = sample_tx();
        let implementation = Address::from([0xaau8; 20]);
        let sig = sign_scw_transaction(&signer, &tx, SignatureMode::RawHash).unwrap();
        let auth = sign_authorization(&signer, tx.chain_id, implementation, tx.nonce).unwrap();

        let built =
            build_7702_transaction(&tx, &sig, auth, 9, 100_000, 2_000_000_000, 1_000_000_000)
                .unwrap();

        assert_eq!(built.chain_id, 943);
        assert_eq!(built.nonce, 9);
        assert_eq!(built.to, tx.account);
        assert_eq!(built.value, U256::ZERO);
        assert_eq!(built.authorization_list.len(), 1);
        // The payload must be execute(txns, signature): selector + args.
        assert_eq!(
            &built.input[..4],
            &crate::encode::encode_execute(&tx.txns, &sig)[..4]
        );
    }

    #[test]
    fn rejects_mismatched_authority() {
        let signer = signer();
        let tx = sample_tx();
        let implementation = Address::from([0xaau8; 20]);
        let sig = sign_scw_transaction(&signer, &tx, SignatureMode::RawHash).unwrap();

        // Sign an authorization for a *different* account key.
        let other = derive_account(&validate_mnemonic(TEST_MNEMONIC).unwrap(), 1).unwrap();
        let auth = sign_authorization(&other, tx.chain_id, implementation, tx.nonce).unwrap();

        assert!(
            build_7702_transaction(&tx, &sig, auth, 9, 100_000, 2_000_000_000, 1_000_000_000)
                .is_err()
        );
    }

    #[test]
    fn rejects_bad_signature_length() {
        let signer = signer();
        let tx = sample_tx();
        let implementation = Address::from([0xaau8; 20]);
        let auth = sign_authorization(&signer, tx.chain_id, implementation, tx.nonce).unwrap();

        assert!(build_7702_transaction(
            &tx,
            &[0u8; 65],
            auth,
            9,
            100_000,
            2_000_000_000,
            1_000_000_000
        )
        .is_err());
    }
}
