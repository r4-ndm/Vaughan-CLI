//! Trezor EOA seams (Phase 2).
//!
//! Module layout (Ledger parity — one concern per file):
//! - [`passphrase`] — optional BIP-39 passphrase (`SecretString`, never logged)
//! - [`ui_bridge`] — Trezor One host PIN matrix ↔ TUI
//! - [`usb`] — blocking `trezor-client` helpers (path parse, connect, sign)
//! - [`session`] — [`DeviceSession`] (USB discover / path → address)
//! - [`backend`] — EVM [`SignerBackend`] (personal / prepared tx; EIP-712 TBD)
//!
//! Rules (same as Ledger):
//! - No fee / RPC / chain registry in this module
//! - No AA / stealth / MCP HID
//! - Confirm-on-device is *in addition to* TUI approve

pub mod backend;
pub mod passphrase;
pub mod session;
pub mod ui_bridge;
pub mod usb;

pub use backend::{
    discover_trezor_account, discover_trezor_account_blocking, preview_trezor_live_paths,
    preview_trezor_live_paths_blocking, trezor_address_for_path, trezor_address_for_path_blocking,
    TrezorSignerBackend,
};
pub use passphrase::TrezorPassphrase;
pub use session::TrezorDeviceSession;
pub use ui_bridge::TrezorUiBridge;
pub use usb::best_effort_host_cancel;
