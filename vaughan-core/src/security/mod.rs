//! Security primitives: HD wallet derivation, vault encryption, ERC-5564 stealth,
//! and hardware-wallet seams (Ledger Phase 1; Trezor Phase 2 USB + PIN matrix).

pub mod encryption;
pub mod hardware;
pub mod hd_wallet;
pub mod signing;
pub mod stealth;

/// Re-export of the BIP-39 [`Mnemonic`] type used throughout the wallet core.
pub use bip39::Mnemonic;

pub use hardware::{
    best_effort_host_cancel, discover_ledger_account, discover_trezor_account,
    discover_trezor_account_blocking, hd_path_from_str, ledger_address_for_path,
    open_hardware_backend, preview_ledger_live_paths, preview_trezor_live_paths,
    preview_trezor_live_paths_blocking, trezor_address_for_path, trezor_address_for_path_blocking,
    AccountKind, DeviceSession, HardwareAccountRecord, HardwareVendor, HwChainFamily,
    LedgerDeviceSession, LedgerSignerBackend, LocalSignerBackend, MockDeviceSession,
    MockSignerBackend, OwnedHardwareBackend, SignRequest, SignResult, SignerBackend,
    TrezorDeviceSession, TrezorPassphrase, TrezorSignerBackend, TrezorUiBridge,
    HARDWARE_INDEX_BASE,
};
