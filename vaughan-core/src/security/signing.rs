//! Off-chain signing: EIP-191 personal messages and EIP-712 typed data.
//!
//! These are the two signing shapes a dApp requests through `personal_sign`
//! and `eth_signTypedData_v4`. Both produce a 65-byte `r || s || v` signature
//! encoded as a `0x`-prefixed hex string — the wire format EIP-1193 clients
//! expect. No network access happens here: a [`PrivateKeySigner`] is handed in
//! by the caller (the wallet derives it from the unlocked mnemonic and drops
//! it immediately after). Signing must only ever be reached *after* the user
//! has approved the request (see `vaughan-provider` / the TUI approval flow).

use alloy::primitives::B256;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use alloy_dyn_abi::TypedData;

use crate::error::WalletError;

/// Sign `message` as an EIP-191 personal message (`\x19Ethereum Signed Message:\n…`).
///
/// `message` is the raw bytes to sign; the caller decides how the dApp's wire
/// value becomes bytes (e.g. decoding `0x`-hex vs. UTF-8). Returns the
/// signature as a `0x`-prefixed hex string (`r || s || v`).
pub fn sign_personal_message(
    signer: &PrivateKeySigner,
    message: &[u8],
) -> Result<String, WalletError> {
    let signature = signer
        .sign_message_sync(message)
        .map_err(|e| WalletError::SigningFailed(e.to_string()))?;
    Ok(encode_signature(signature.as_bytes()))
}

/// Sign a raw 32-byte hash, returning the 65-byte `r ‖ s ‖ v` signature as
/// `0x`-prefixed hex.
///
/// This is *not* EIP-191/EIP-712: the digest is signed as-is, with no prefix or
/// domain separator. Call it only when the caller has already computed the exact
/// digest a verifier will recover against (e.g. an Ambire smart-account batch).
/// For user-facing messages prefer [`sign_personal_message`] or
/// [`sign_typed_data`].
pub fn sign_hash(signer: &PrivateKeySigner, hash: &B256) -> Result<String, WalletError> {
    let signature = signer
        .sign_hash_sync(hash)
        .map_err(|e| WalletError::SigningFailed(e.to_string()))?;
    Ok(encode_signature(signature.as_bytes()))
}

/// Sign an EIP-712 typed-data payload (`{types, primaryType, domain, message}`).
///
/// The payload is parsed dynamically via [`TypedData`] so arbitrary dApp
/// schemas sign correctly without compile-time types. Returns the signature as
/// a `0x`-prefixed hex string (`r || s || v`).
pub fn sign_typed_data(
    signer: &PrivateKeySigner,
    typed_data: &serde_json::Value,
) -> Result<String, WalletError> {
    let typed_data: TypedData = serde_json::from_value(typed_data.clone())
        .map_err(|e| WalletError::InvalidTransaction(format!("invalid EIP-712 typed data: {e}")))?;
    let hash = typed_data
        .eip712_signing_hash()
        .map_err(|e| WalletError::SigningFailed(format!("EIP-712 hash failed: {e}")))?;
    let signature = signer
        .sign_hash_sync(&hash)
        .map_err(|e| WalletError::SigningFailed(e.to_string()))?;
    Ok(encode_signature(signature.as_bytes()))
}

/// Hex-encode a 65-byte signature as `0x` + `r || s || v`.
fn encode_signature(bytes: [u8; 65]) -> String {
    format!("0x{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::hd_wallet::{derive_account, validate_mnemonic};
    use alloy::primitives::Address;

    // Canonical BIP-39/BIP-44 test vector (all-"abandon" mnemonic).
    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn signer() -> PrivateKeySigner {
        derive_account(&validate_mnemonic(TEST_MNEMONIC).unwrap(), 0).unwrap()
    }

    #[test]
    fn personal_sign_is_eip191_prefixed_and_recoverable() {
        let signer = signer();
        let sig_hex = sign_personal_message(&signer, b"hello").unwrap();
        assert!(sig_hex.starts_with("0x"));
        let bytes = hex::decode(&sig_hex[2..]).unwrap();
        assert_eq!(bytes.len(), 65, "signature must be r||s||v (65 bytes)");

        // The signature must recover to the signer's address.
        let sig = alloy::primitives::Signature::from_raw(bytes.as_slice()).unwrap();
        let recovered: Address = sig.recover_address_from_msg(b"hello").unwrap();
        assert_eq!(recovered, signer.address());
    }

    #[test]
    fn personal_sign_is_deterministic_for_same_input() {
        let signer = signer();
        assert_eq!(
            sign_personal_message(&signer, b"same").unwrap(),
            sign_personal_message(&signer, b"same").unwrap()
        );
    }

    #[test]
    fn typed_data_signs_standard_payload() {
        let signer = signer();
        let payload = serde_json::json!({
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"}
                ],
                "Message": [
                    {"name": "hello", "type": "string"}
                ]
            },
            "primaryType": "Message",
            "domain": {"name": "Example"},
            "message": {"hello": "world"},
        });
        let sig_hex = sign_typed_data(&signer, &payload).unwrap();
        assert!(sig_hex.starts_with("0x"));
        assert_eq!(hex::decode(&sig_hex[2..]).unwrap().len(), 65);
    }

    #[test]
    fn typed_data_rejects_malformed_payload() {
        let signer = signer();
        // Missing `types`/`primaryType` cannot be resolved.
        assert!(sign_typed_data(&signer, &serde_json::json!({"message": {}})).is_err());
    }
}
