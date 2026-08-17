//! Security primitives: HD wallet derivation and vault encryption.

pub mod encryption;
pub mod hd_wallet;

/// Re-export of the BIP-39 [`Mnemonic`] type used throughout the wallet core.
pub use bip39::Mnemonic;
