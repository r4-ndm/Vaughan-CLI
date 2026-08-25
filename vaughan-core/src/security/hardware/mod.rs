//! Hardware wallet seams: modular, multichain-ready signing.
//!
//! Layers (see `docs/hardware-wallets.md`):
//! - [`types`] — watch records + family-tagged [`SignRequest`] / [`SignResult`]
//! - [`SignerBackend`] / [`LocalSignerBackend`] — wallet-facing async surface
//! - [`DeviceSession`] — vendor USB contract
//! - [`ledger`] — Ledger HID (Phase 1)
//! - [`mock`] — Anvil/CI stand-in (no USB)
//! - [`profiles`] — per-family encode/sign helpers (EVM first)

pub mod backend;
pub mod ledger;
pub mod mock;
pub mod profiles;
pub mod session;
pub mod types;

pub use backend::{LocalSignerBackend, SignerBackend};
pub use ledger::{
    discover_ledger_account, hd_path_from_str, ledger_address_for_path, preview_ledger_live_paths,
    LedgerDeviceSession, LedgerSignerBackend,
};
pub use mock::{MockDeviceSession, MockSignerBackend};
pub use session::DeviceSession;
pub use types::{
    AccountKind, HardwareAccountRecord, HardwareVendor, HwChainFamily, SignRequest, SignResult,
    HARDWARE_INDEX_BASE,
};
