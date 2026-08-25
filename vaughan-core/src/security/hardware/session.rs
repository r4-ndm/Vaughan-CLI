//! Vendor-agnostic device session contract (USB/HID).
//!
//! Phase 0 ships the trait only — no Ledger/Trezor crates. Implementations must
//! not embed EIP-1559 builders or RPC; they sign preimages / return addresses.

use async_trait::async_trait;

use super::types::{HardwareVendor, HwChainFamily};
use crate::error::WalletError;

/// Open device → path → address / raw sign. Chain profiles sit above this.
#[async_trait]
pub trait DeviceSession: Send + Sync {
    fn vendor(&self) -> HardwareVendor;

    /// Preview `(derivation_path, address)` pairs for `family` (device app open).
    async fn list_paths_preview(
        &self,
        family: HwChainFamily,
    ) -> Result<Vec<(String, String)>, WalletError>;

    /// Address at `path` for `family` (confirm-on-device may be required later).
    async fn address_for_path(
        &self,
        family: HwChainFamily,
        path: &str,
    ) -> Result<String, WalletError>;

    /// Sign a family-specific preimage at `path` (exact meaning is profile-defined).
    async fn sign_preimage(
        &self,
        family: HwChainFamily,
        path: &str,
        preimage: &[u8],
    ) -> Result<Vec<u8>, WalletError>;
}
