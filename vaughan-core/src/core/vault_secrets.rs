//! Encrypted vault plaintext layout (mnemonic + optional imported keys).
//!
//! Legacy vaults store a raw BIP-39 phrase. New writes use a small JSON envelope
//! so imported private keys can ride inside the same Argon2id + AES-GCM blob.
//! Secrets are zeroized after encrypt/decrypt; never log this plaintext.

use alloy::signers::local::PrivateKeySigner;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use zeroize::Zeroize;

use crate::error::WalletError;

/// Versioned secrets payload stored inside [`EncryptedVault`] ciphertext.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultSecrets {
    pub mnemonic: String,
    #[serde(default)]
    pub imported: Vec<ImportedKeyRecord>,
}

/// One imported EOA private key (hex), kept only inside the encrypted vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedKeyRecord {
    pub label: String,
    /// Hex private key (`0x`-optional). Zeroized when the parent payload is.
    pub private_key: String,
}

impl VaultSecrets {
    pub fn from_mnemonic_phrase(phrase: impl Into<String>) -> Self {
        Self {
            mnemonic: phrase.into(),
            imported: Vec::new(),
        }
    }

    /// Parse legacy (raw mnemonic) or JSON envelope plaintext from the vault.
    pub fn decode(plaintext: &str) -> Result<Self, WalletError> {
        let trimmed = plaintext.trim();
        if trimmed.starts_with('{') {
            serde_json::from_str(trimmed).map_err(|e| {
                WalletError::DecryptionFailed(format!("vault secrets JSON is invalid: {e}"))
            })
        } else {
            Ok(Self::from_mnemonic_phrase(trimmed.to_string()))
        }
    }

    /// Serialize for encryption. Prefer JSON so imported keys survive re-lock.
    pub fn encode(&self) -> Result<String, WalletError> {
        if self.imported.is_empty() {
            // Keep legacy single-phrase form when nothing is imported so
            // existing tooling that peeks at fixtures stays simple.
            return Ok(self.mnemonic.clone());
        }
        serde_json::to_string(self)
            .map_err(|e| WalletError::EncryptionFailed(format!("vault secrets encode: {e}")))
    }

    pub fn zeroize(&mut self) {
        self.mnemonic.zeroize();
        for key in &mut self.imported {
            key.private_key.zeroize();
            key.label.zeroize();
        }
        self.imported.clear();
    }
}

impl Drop for VaultSecrets {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// Parse a hex private key into a signer; never put the raw key into the error.
pub fn parse_private_key(raw: &str) -> Result<PrivateKeySigner, WalletError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(WalletError::InvalidPrivateKey("empty private key".into()));
    }
    PrivateKeySigner::from_str(trimmed)
        .map_err(|_| WalletError::InvalidPrivateKey("expected a 32-byte hex private key".into()))
}

/// Hex-encode a signer's private key for export (caller wraps in [`SecretString`]).
pub fn private_key_hex(signer: &PrivateKeySigner) -> SecretString {
    let bytes = signer.to_bytes();
    let hex = format!("0x{}", hex::encode(bytes));
    SecretString::new(hex)
}

/// Validate that `password` decrypts the current vault (wrong password → error).
pub fn assert_password(
    vault: &crate::security::encryption::EncryptedVault,
    password: &SecretString,
) -> Result<(), WalletError> {
    let mut plaintext = crate::security::encryption::decrypt(vault, password)?;
    // Touch the bytes so a wrong password still fails the same path.
    let _ = plaintext.first();
    plaintext.zeroize();
    let _ = password.expose_secret();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_phrase_roundtrip() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let secrets = VaultSecrets::decode(phrase).unwrap();
        assert_eq!(secrets.mnemonic, phrase);
        assert!(secrets.imported.is_empty());
        assert_eq!(secrets.encode().unwrap(), phrase);
    }

    #[test]
    fn json_with_imported_roundtrip() {
        let mut secrets = VaultSecrets::from_mnemonic_phrase("test phrase");
        secrets.imported.push(ImportedKeyRecord {
            label: "hot".into(),
            private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                .into(),
        });
        let encoded = secrets.encode().unwrap();
        assert!(encoded.starts_with('{'));
        let back = VaultSecrets::decode(&encoded).unwrap();
        assert_eq!(back.imported.len(), 1);
        assert_eq!(back.imported[0].label, "hot");
    }

    #[test]
    fn parse_anvil_dev_key() {
        let signer =
            parse_private_key("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80")
                .unwrap();
        assert_eq!(
            format!("{:#x}", signer.address()),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }
}
