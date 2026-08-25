//! Security primitives: HD wallet derivation, vault encryption, ERC-5564 stealth,
//! and hardware-wallet seams (Phase 0 — no HID crates).

pub mod encryption;
pub mod hardware;
pub mod hd_wallet;
pub mod signing;
pub mod stealth;

/// Re-export of the BIP-39 [`Mnemonic`] type used throughout the wallet core.
pub use bip39::Mnemonic;

pub use hardware::{
    AccountKind, DeviceSession, HardwareAccountRecord, HardwareVendor, HwChainFamily,
    LocalSignerBackend, SignRequest, SignResult, SignerBackend, HARDWARE_INDEX_BASE,
};
