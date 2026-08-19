//! Security primitives: HD wallet derivation, vault encryption, ERC-5564 stealth.

pub mod encryption;
pub mod hd_wallet;
pub mod signing;
pub mod stealth;

/// Re-export of the BIP-39 [`Mnemonic`] type used throughout the wallet core.
pub use bip39::Mnemonic;
