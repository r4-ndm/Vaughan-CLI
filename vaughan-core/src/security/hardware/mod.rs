//! Hardware wallet seams: modular, multichain-ready signing (no HID in Phase 0).
//!
//! Layers (see `docs/hardware-wallets.md`):
//! - [`types`] — watch records + family-tagged [`SignRequest`] / [`SignResult`]
//! - [`SignerBackend`] / [`LocalSignerBackend`] — wallet-facing async surface
//! - [`DeviceSession`] — vendor USB contract (trait only until Phase 1)
//! - [`profiles`] — per-family encode/sign helpers (EVM first)

pub mod backend;
pub mod profiles;
pub mod session;
pub mod types;

pub use backend::{LocalSignerBackend, SignerBackend};
pub use session::DeviceSession;
pub use types::{
    AccountKind, HardwareAccountRecord, HardwareVendor, HwChainFamily, SignRequest, SignResult,
    HARDWARE_INDEX_BASE,
};
