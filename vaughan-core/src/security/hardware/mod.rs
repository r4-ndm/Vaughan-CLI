//! Hardware wallet seams: modular, multichain-ready signing.
//!
//! Layers (see `docs/hardware-wallets.md`):
//! - [`types`] — watch records + family-tagged [`SignRequest`] / [`SignResult`]
//! - [`paths`] — shared BIP-44 EVM path strings (vendor-agnostic)
//! - [`SignerBackend`] / [`LocalSignerBackend`] — wallet-facing async surface
//! - [`factory`] — vendor dispatch (`open_hardware_backend`)
//! - [`DeviceSession`] — vendor USB contract
//! - [`ledger`] — Ledger HID (Phase 1)
//! - [`trezor`] — Trezor USB + Trezor One PIN matrix bridge (Phase 2)
//! - [`mock`] — Anvil/CI stand-in (no USB)
//! - [`profiles`] — per-family encode/sign helpers (EVM first)
//!
//! Layering note: this module imports `crate::chains::EvmTransaction` — a
//! deliberate, types-only exception to the chains/security split. The signer
//! consumes the transaction *description* (a plain serde struct from
//! `chains::types`), never chain behaviour; the dependency is one-directional
//! (`chains` never imports `security`).

pub mod backend;
pub mod factory;
pub mod ledger;
pub mod mock;
pub mod paths;
pub mod profiles;
pub mod session;
pub mod trezor;
pub mod types;

pub use backend::{LocalSignerBackend, SignerBackend};
pub use factory::{open_hardware_backend, OwnedHardwareBackend};
pub use ledger::{
    discover_ledger_account, hd_path_from_str, ledger_address_for_path, preview_ledger_live_paths,
    LedgerDeviceSession, LedgerSignerBackend,
};
pub use mock::{MockDeviceSession, MockSignerBackend};
pub use paths::{evm_ledger_live_path, evm_live_preview_paths, evm_standard_path};
pub use session::DeviceSession;
pub use trezor::{
    discover_trezor_account, discover_trezor_account_blocking, preview_trezor_live_paths,
    preview_trezor_live_paths_blocking, trezor_address_for_path, trezor_address_for_path_blocking,
    TrezorDeviceSession, TrezorPassphrase, TrezorSignerBackend, TrezorUiBridge,
};
pub use types::{
    AccountKind, HardwareAccountRecord, HardwareVendor, HwChainFamily, SignRequest, SignResult,
    HARDWARE_INDEX_BASE,
};
