//! HD wallet: BIP-39 mnemonic generation/validation and BIP-32/44 derivation.
//!
//! Mnemonics are handled by the [`bip39`] crate (12-word English) and key
//! derivation by the RustCrypto [`bip32`] crate. The derived key is handed to
//! Alloy as a [`PrivateKeySigner`]. The `bip39` `zeroize` feature is enabled so
//! mnemonics are zeroized on drop.

use std::str::FromStr;

use alloy::signers::local::PrivateKeySigner;
use bip32::{ChildNumber, DerivationPath, XPrv};
use bip39::{Language, Mnemonic};

use crate::error::WalletError;

/// Ethereum (BIP-44 coin type 60) account derivation path prefix.
pub const ETH_DERIVATION_PATH: &str = "m/44'/60'/0'/0";

/// Generate a new 12-word English mnemonic.
///
/// Uses a cryptographically secure RNG via the `bip39` crate.
pub fn generate_mnemonic() -> Result<Mnemonic, WalletError> {
    Mnemonic::generate_in(Language::English, 12)
        .map_err(|e| WalletError::Other(format!("mnemonic generation failed: {e}")))
}

/// Validate a BIP-39 mnemonic phrase (whitespace-insensitive).
///
/// Returns the parsed mnemonic so callers can derive keys without re-parsing.
pub fn validate_mnemonic(phrase: &str) -> Result<Mnemonic, WalletError> {
    let normalized = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
    Mnemonic::parse_in_normalized(Language::English, &normalized).map_err(|_| {
        WalletError::InvalidMnemonic("recovery phrase is not a valid BIP-39 mnemonic".to_string())
    })
}

/// Derive the hardened parent key at `m/44'/60'/0'/0`.
///
/// This is the expensive step (PBKDF2 over the mnemonic + the hardened BIP-32
/// path); child accounts derive cheaply from the returned parent, so callers
/// that need several accounts should derive the parent once and reuse it.
pub fn derive_account_parent(mnemonic: &Mnemonic) -> Result<XPrv, WalletError> {
    let seed = mnemonic.to_seed("");
    let path = DerivationPath::from_str(ETH_DERIVATION_PATH)
        .map_err(|e| WalletError::KeyDerivationFailed(e.to_string()))?;
    XPrv::derive_from_path(seed, &path).map_err(|e| WalletError::KeyDerivationFailed(e.to_string()))
}

/// Derive account `index` from an already-derived [`derive_account_parent`]
/// parent key (the non-hardened child step).
pub fn derive_account_from_parent(
    parent: &XPrv,
    index: u32,
) -> Result<PrivateKeySigner, WalletError> {
    let child = ChildNumber::new(index, false)
        .map_err(|e| WalletError::KeyDerivationFailed(e.to_string()))?;
    let xprv = parent
        .derive_child(child)
        .map_err(|e| WalletError::KeyDerivationFailed(e.to_string()))?;
    Ok(PrivateKeySigner::from_signing_key(
        xprv.private_key().clone(),
    ))
}

/// Derive the Ethereum account signer at `m/44'/60'/0'/0/{index}`.
pub fn derive_account(mnemonic: &Mnemonic, index: u32) -> Result<PrivateKeySigner, WalletError> {
    derive_account_from_parent(&derive_account_parent(mnemonic)?, index)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Canonical BIP-39/BIP-44 test vector (all-"abandon" mnemonic).
    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    // Expected Ethereum account 0 address for the vector above.
    const TEST_ADDRESS_0: &str = "0x9858effd232b4033e47d90003d41ec34ecaeda94";

    #[test]
    fn generates_valid_12_word_mnemonic() {
        let m = generate_mnemonic().unwrap();
        assert_eq!(m.word_count(), 12);
        assert!(validate_mnemonic(&m.to_string()).is_ok());
    }

    #[test]
    fn validates_and_normalizes_whitespace() {
        let m = validate_mnemonic(&format!("  {TEST_MNEMONIC}  ")).unwrap();
        assert_eq!(m.to_string(), TEST_MNEMONIC);
    }

    #[test]
    fn rejects_invalid_mnemonic() {
        assert!(validate_mnemonic("not a valid mnemonic phrase").is_err());
    }

    #[test]
    fn derives_known_eth_account_0() {
        let m = validate_mnemonic(TEST_MNEMONIC).unwrap();
        let signer = derive_account(&m, 0).unwrap();
        assert_eq!(signer.address().to_string().to_lowercase(), TEST_ADDRESS_0);
    }

    #[test]
    fn derives_distinct_accounts() {
        let m = validate_mnemonic(TEST_MNEMONIC).unwrap();
        assert_ne!(
            derive_account(&m, 0).unwrap().address(),
            derive_account(&m, 1).unwrap().address()
        );
    }
}
