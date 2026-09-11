//! Trezor [`DeviceSession`] — USB discovery / path preview.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::security::hardware::session::DeviceSession;
use crate::security::hardware::types::{HardwareVendor, HwChainFamily};

use super::ui_bridge::TrezorUiBridge;
use super::usb;

/// Open Trezor session for path probes (Ethereum capability on device).
pub struct TrezorDeviceSession {
    chain_id: Option<u64>,
    ui: Option<Arc<TrezorUiBridge>>,
}

impl TrezorDeviceSession {
    pub async fn connect(
        chain_id: Option<u64>,
        ui: Option<Arc<TrezorUiBridge>>,
    ) -> Result<Self, WalletError> {
        let ui_probe = ui.clone();
        // Probe USB once so callers get a clear error before preview loops.
        tokio::task::spawn_blocking(move || {
            let _ = usb::open_initialized(None, ui_probe.as_ref())?;
            Ok::<(), WalletError>(())
        })
        .await
        .map_err(|e| WalletError::SigningFailed(format!("Trezor task: {e}")))??;
        Ok(Self { chain_id, ui })
    }
}

#[async_trait]
impl DeviceSession for TrezorDeviceSession {
    fn vendor(&self) -> HardwareVendor {
        HardwareVendor::Trezor
    }

    async fn list_paths_preview(
        &self,
        family: HwChainFamily,
    ) -> Result<Vec<(String, String)>, WalletError> {
        if !matches!(family, HwChainFamily::Evm) {
            return Err(WalletError::HardwareUnsupported(
                "Trezor Phase 2 only supports EVM paths".into(),
            ));
        }
        super::backend::preview_trezor_live_paths(5, self.chain_id, self.ui.clone()).await
    }

    async fn address_for_path(
        &self,
        family: HwChainFamily,
        path: &str,
    ) -> Result<String, WalletError> {
        if !matches!(family, HwChainFamily::Evm) {
            return Err(WalletError::HardwareUnsupported(
                "Trezor Phase 2 only supports EVM paths".into(),
            ));
        }
        super::backend::trezor_address_for_path(path, self.chain_id, self.ui.clone()).await
    }

    async fn sign_preimage(
        &self,
        _family: HwChainFamily,
        _path: &str,
        _preimage: &[u8],
    ) -> Result<Vec<u8>, WalletError> {
        Err(WalletError::HardwareUnsupported(
            "Trezor does not expose raw preimage signing — use TrezorSignerBackend".into(),
        ))
    }
}
