//! Security primitives: HD wallet derivation, vault encryption, ERC-5564 stealth,
//! and hardware-wallet seams (Ledger Phase 1).

pub mod encryption;
pub mod hardware;
pub mod hd_wallet;
pub mod signing;
pub mod stealth;

/// Re-export of the BIP-39 [`Mnemonic`] type used throughout the wallet core.
pub use bip39::Mnemonic;

pub use hardware::{
    discover_ledger_account, hd_path_from_str, ledger_address_for_path, preview_ledger_live_paths,
    AccountKind, DeviceSession, HardwareAccountRecord, HardwareVendor, HwChainFamily,
    LedgerDeviceSession, LedgerSignerBackend, LocalSignerBackend, MockDeviceSession,
    MockSignerBackend, SignRequest, SignResult, SignerBackend, HARDWARE_INDEX_BASE,
};
