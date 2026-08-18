//! Sign the Ambire digest and assemble the 66-byte signature.

use alloy::signers::local::PrivateKeySigner;

use vaughan_core::error::WalletError;
use vaughan_core::security::signing;

use crate::scw::{ScwTransaction, SignatureMode};

/// Sign `tx`, returning the 66-byte `r ‖ s ‖ v ‖ mode` signature the on-chain
/// `AmbireAccount.execute` expects.
///
/// `mode` selects the signing scheme for the digest
/// `keccak256(abi.encode(account, chainId, nonce, txns))`:
/// [`SignatureMode::RawHash`] signs the digest directly, [`SignatureMode::EthSign`]
/// signs EIP-191 `"\x19Ethereum Signed Message:\n32" ‖ digest`. Either way the
/// one-byte mode is appended as the final byte.
pub fn sign_scw_transaction(
    signer: &PrivateKeySigner,
    tx: &ScwTransaction,
    mode: SignatureMode,
) -> Result<Vec<u8>, WalletError> {
    let digest = tx.digest();
    let r_s_v_hex = match mode {
        SignatureMode::RawHash => signing::sign_hash(signer, &digest)?,
        SignatureMode::EthSign => signing::sign_personal_message(signer, digest.as_slice())?,
    };
    let mut bytes = hex::decode(r_s_v_hex.trim_start_matches("0x"))
        .map_err(|e| WalletError::SigningFailed(e.to_string()))?;
    debug_assert_eq!(bytes.len(), 65);
    bytes.push(mode as u8);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::Transaction;
    use alloy::primitives::{Address, Bytes, U256};
    use vaughan_core::security::hd_wallet::{derive_account, validate_mnemonic};

    // Canonical BIP-39/BIP-44 test vector (all-"abandon" mnemonic).
    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn signer() -> PrivateKeySigner {
        derive_account(&validate_mnemonic(TEST_MNEMONIC).unwrap(), 0).unwrap()
    }

    fn sample_tx() -> ScwTransaction {
        ScwTransaction {
            account: Address::from([0x11u8; 20]),
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
    fn raw_hash_signature_is_66_bytes_and_recovers() {
        let signer = signer();
        let tx = sample_tx();
        let sig = sign_scw_transaction(&signer, &tx, SignatureMode::RawHash).unwrap();

        assert_eq!(sig.len(), 66);
        assert_eq!(sig[65], SignatureMode::RawHash as u8);

        let alloy_sig = alloy::primitives::Signature::from_raw(&sig[..65]).unwrap();
        let recovered = alloy_sig
            .recover_address_from_prehash(&tx.digest())
            .unwrap();
        assert_eq!(recovered, signer.address());
    }

    #[test]
    fn ethsign_signature_recovers_via_personal_message() {
        let signer = signer();
        let tx = sample_tx();
        let sig = sign_scw_transaction(&signer, &tx, SignatureMode::EthSign).unwrap();

        assert_eq!(sig.len(), 66);
        assert_eq!(sig[65], SignatureMode::EthSign as u8);

        let alloy_sig = alloy::primitives::Signature::from_raw(&sig[..65]).unwrap();
        let recovered = alloy_sig
            .recover_address_from_msg(tx.digest().as_slice())
            .unwrap();
        assert_eq!(recovered, signer.address());
    }
}
