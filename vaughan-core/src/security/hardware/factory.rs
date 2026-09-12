//! Vendor dispatch for hardware [`SignerBackend`]s.
//!
//! `WalletState` (and tests) open a backend here — never `match` on vendor HID
//! details outside this module / the vendor files.

use std::sync::Arc;

use super::ledger::LedgerSignerBackend;
use super::mock::MockSignerBackend;
use super::trezor::{TrezorSignerBackend, TrezorUiBridge};
use super::types::{HardwareAccountRecord, HardwareVendor, SignRequest, SignResult};
use super::SignerBackend;
use crate::error::WalletError;

/// Owned hardware backend for the active watch account (plus Anvil mock).
pub enum OwnedHardwareBackend {
    Ledger(LedgerSignerBackend),
    Trezor(TrezorSignerBackend),
    Mock(MockSignerBackend),
}

impl OwnedHardwareBackend {
    pub async fn sign(&self, req: SignRequest) -> Result<SignResult, WalletError> {
        match self {
            Self::Ledger(b) => b.sign(req).await,
            Self::Trezor(b) => b.sign(req).await,
            Self::Mock(b) => b.sign(req).await,
        }
    }

    pub fn address(&self) -> &str {
        match self {
            Self::Ledger(b) => b.address(),
            Self::Trezor(b) => b.address(),
            Self::Mock(b) => b.address(),
        }
    }
}

/// Open the USB (or mock) backend for a persisted watch record.
///
/// `mock`, when set, short-circuits USB for CI/Anvil (address must match).
/// `trezor_ui` supplies the Trezor One host PIN matrix (required for Model One).
pub fn open_hardware_backend(
    record: &HardwareAccountRecord,
    chain_id: Option<u64>,
    mock: Option<&MockSignerBackend>,
    trezor_ui: Option<Arc<TrezorUiBridge>>,
) -> Result<OwnedHardwareBackend, WalletError> {
    if let Some(mock) = mock {
        if !mock.address_string().eq_ignore_ascii_case(&record.address) {
            return Err(WalletError::HardwareUnsupported(
                "hardware mock address does not match active watch account".into(),
            ));
        }
        return Ok(OwnedHardwareBackend::Mock(mock.clone()));
    }
    match record.vendor {
        HardwareVendor::Ledger => Ok(OwnedHardwareBackend::Ledger(LedgerSignerBackend::new(
            record.clone(),
            chain_id,
        )?)),
        HardwareVendor::Trezor => Ok(OwnedHardwareBackend::Trezor(TrezorSignerBackend::with_ui(
            record.clone(),
            chain_id,
            None,
            trezor_ui,
        )?)),
    }
}
