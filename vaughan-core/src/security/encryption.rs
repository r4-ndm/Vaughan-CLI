//! Vault encryption: Argon2id key derivation + AES-256-GCM authenticated encryption.
//!
//! A password is stretched with Argon2id into a 32-byte key, which encrypts the
//! mnemonic with AES-256-GCM. Only the salt, nonce, and ciphertext are persisted
//! — never the password or plaintext.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::rngs::OsRng;
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::error::WalletError;

/// Salt length for Argon2id (bytes).
const SALT_LEN: usize = 16;
/// Nonce length for AES-256-GCM (bytes).
const NONCE_LEN: usize = 12;
/// Derived key length (AES-256).
const KEY_LEN: usize = 32;

/// Production Argon2id cost parameters (64 MiB, 3 iterations, 4 lanes).
/// Only referenced outside tests (the test preset is inline), so silence the
/// test-build `dead_code` lint.
#[cfg_attr(test, allow(dead_code))]
const PROD_M_COST_KIB: u32 = 65_536;
#[cfg_attr(test, allow(dead_code))]
const PROD_T_COST: u32 = 3;
#[cfg_attr(test, allow(dead_code))]
const PROD_P_COST: u32 = 4;

/// An encrypted vault payload. All fields are hex-encoded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedVault {
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

/// Enforce the password strength policy (FR-1.4): >= 12 chars with upper,
/// lower, digit, and symbol.
pub fn validate_password_policy(password: &SecretString) -> Result<(), WalletError> {
    let p = password.expose_secret();
    if p.chars().count() < 12 {
        return Err(WalletError::PasswordPolicy(
            "Password must be at least 12 characters.".to_string(),
        ));
    }
    let (mut upper, mut lower, mut digit, mut symbol) = (false, false, false, false);
    for c in p.chars() {
        if c.is_ascii_uppercase() {
            upper = true;
        } else if c.is_ascii_lowercase() {
            lower = true;
        } else if c.is_ascii_digit() {
            digit = true;
        } else {
            symbol = true;
        }
    }
    if !(upper && lower && digit && symbol) {
        return Err(WalletError::PasswordPolicy(
            "Password must include uppercase, lowercase, a digit, and a symbol.".to_string(),
        ));
    }
    Ok(())
}

/// Concrete AES-256-GCM nonce type (12 bytes).
type GcmNonce = Nonce<<Aes256Gcm as AeadCore>::NonceSize>;

/// Build a 12-byte AES-GCM nonce from raw bytes.
///
/// `#[allow(deprecated)]` is required because aes-gcm 0.10 pins generic-array
/// 0.14, whose `from_slice` is deprecated in favor of 1.x (which aes-gcm cannot
/// adopt yet).
#[allow(deprecated)]
fn nonce_from_bytes(bytes: &[u8]) -> GcmNonce {
    *GcmNonce::from_slice(bytes)
}

/// Argon2id parameters, with a fast `#[cfg(test)]` preset (see CLAUDE.md
/// security guardrail #3 — never weaken KDF costs outside tests).
fn kdf_params() -> Params {
    #[cfg(test)]
    let (m, t, p) = (1_024, 1, 1);
    #[cfg(not(test))]
    let (m, t, p) = (PROD_M_COST_KIB, PROD_T_COST, PROD_P_COST);

    Params::new(m, t, p, Some(KEY_LEN)).expect("valid Argon2id parameters")
}

/// Encrypt `plaintext` under `password` (Argon2id -> AES-256-GCM).
pub fn encrypt(plaintext: &[u8], password: &SecretString) -> Result<EncryptedVault, WalletError> {
    validate_password_policy(password)?;

    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);

    let mut key = [0u8; KEY_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, kdf_params())
        .hash_password_into(password.expose_secret().as_bytes(), &salt, &mut key)
        .map_err(|e| WalletError::EncryptionFailed(e.to_string()))?;

    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| WalletError::EncryptionFailed(e.to_string()))?;
    let nonce = nonce_from_bytes(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| WalletError::EncryptionFailed(e.to_string()))?;

    key.zeroize();

    Ok(EncryptedVault {
        salt: hex::encode(salt),
        nonce: hex::encode(nonce_bytes),
        ciphertext: hex::encode(ciphertext),
    })
}

/// Decrypt `vault` under `password`, returning the plaintext.
pub fn decrypt(vault: &EncryptedVault, password: &SecretString) -> Result<Vec<u8>, WalletError> {
    let salt = hex::decode(&vault.salt)
        .map_err(|_| WalletError::DecryptionFailed("invalid salt".to_string()))?;
    let nonce_bytes = hex::decode(&vault.nonce)
        .map_err(|_| WalletError::DecryptionFailed("invalid nonce".to_string()))?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(WalletError::DecryptionFailed(
            "invalid nonce length".to_string(),
        ));
    }
    let ciphertext = hex::decode(&vault.ciphertext)
        .map_err(|_| WalletError::DecryptionFailed("invalid ciphertext".to_string()))?;

    let mut key = [0u8; KEY_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, kdf_params())
        .hash_password_into(password.expose_secret().as_bytes(), &salt, &mut key)
        .map_err(|e| WalletError::DecryptionFailed(e.to_string()))?;

    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| WalletError::DecryptionFailed(e.to_string()))?;
    let nonce = nonce_from_bytes(&nonce_bytes);
    let plaintext = cipher.decrypt(&nonce, ciphertext.as_ref()).map_err(|_| {
        WalletError::DecryptionFailed("wrong password or corrupted vault".to_string())
    })?;

    key.zeroize();
    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strong_password() -> SecretString {
        SecretString::from("CorrectHorse9!BatteryStaple".to_string())
    }

    #[test]
    fn password_policy_enforced() {
        assert!(validate_password_policy(&strong_password()).is_ok());
        assert!(validate_password_policy(&SecretString::from("short".to_string())).is_err());
        assert!(
            validate_password_policy(&SecretString::from("alllowercase123!".to_string())).is_err()
        );
        assert!(
            validate_password_policy(&SecretString::from("ALLUPPERCASE123!".to_string())).is_err()
        );
        assert!(
            validate_password_policy(&SecretString::from("NoDigitsHere!".to_string())).is_err()
        );
        assert!(validate_password_policy(&SecretString::from("NoSymbol123".to_string())).is_err());
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let pw = strong_password();
        let vault = encrypt(plaintext, &pw).unwrap();
        // Ciphertext must not contain the plaintext.
        assert_ne!(vault.ciphertext, hex::encode(plaintext));
        assert_eq!(decrypt(&vault, &pw).unwrap(), plaintext);
    }

    #[test]
    fn wrong_password_fails() {
        let pw = strong_password();
        let vault = encrypt(b"secret data", &pw).unwrap();
        let wrong = SecretString::from("WrongPassword9!BatteryStaple".to_string());
        assert!(decrypt(&vault, &wrong).is_err());
    }
}
