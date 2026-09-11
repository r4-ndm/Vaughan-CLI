//! Trezor EVM [`SignerBackend`] — USB via `trezor-client`.

use std::sync::Arc;

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};

use crate::error::WalletError;
use crate::security::hardware::backend::SignerBackend;
use crate::security::hardware::paths::evm_live_preview_paths;
use crate::security::hardware::types::{
    HardwareAccountRecord, HardwareVendor, HwChainFamily, SignRequest, SignResult,
};

use super::ui_bridge::TrezorUiBridge;
use super::usb;

/// Watch-record-backed Trezor signer (confirm-on-device / PIN matrix).
pub struct TrezorSignerBackend {
    record: HardwareAccountRecord,
    #[allow(dead_code)]
    chain_id: Option<u64>,
    passphrase: Option<SecretString>,
    ui: Option<Arc<TrezorUiBridge>>,
}

impl TrezorSignerBackend {
    pub fn new(record: HardwareAccountRecord, chain_id: Option<u64>) -> Result<Self, WalletError> {
        Self::with_ui(record, chain_id, None, None)
    }

    pub fn with_ui(
        record: HardwareAccountRecord,
        chain_id: Option<u64>,
        passphrase: Option<SecretString>,
        ui: Option<Arc<TrezorUiBridge>>,
    ) -> Result<Self, WalletError> {
        if record.vendor != HardwareVendor::Trezor {
            return Err(WalletError::HardwareUnsupported(
                "expected a Trezor watch account".into(),
            ));
        }
        if !matches!(record.family, HwChainFamily::Evm) {
            return Err(WalletError::HardwareUnsupported(
                "Trezor Phase 2 is EVM-only".into(),
            ));
        }
        Ok(Self {
            record,
            chain_id,
            passphrase,
            ui,
        })
    }

    pub fn record(&self) -> &HardwareAccountRecord {
        &self.record
    }

    fn passphrase_for_task(&self) -> Option<SecretString> {
        self.passphrase
            .as_ref()
            .map(|s| SecretString::new(s.expose_secret().clone()))
    }
}

#[async_trait]
impl SignerBackend for TrezorSignerBackend {
    fn address(&self) -> &str {
        &self.record.address
    }

    fn family(&self) -> HwChainFamily {
        HwChainFamily::Evm
    }

    async fn sign(&self, req: SignRequest) -> Result<SignResult, WalletError> {
        let path = self.record.derivation_path.clone();
        let expected = self.record.address.clone();
        let passphrase = self.passphrase_for_task();
        let ui = self.ui.clone();

        match req {
            SignRequest::EvmPersonal { message } => {
                let hex = tokio::task::spawn_blocking(move || {
                    let mut device = usb::open_initialized(passphrase.as_ref(), ui.as_ref())?;
                    let got = usb::ethereum_address(&mut device, &path, ui.as_ref())?;
                    if !got.eq_ignore_ascii_case(&expected) {
                        return Err(WalletError::HardwareUnsupported(format!(
                            "Trezor address {got} does not match watch record {expected}"
                        )));
                    }
                    usb::ethereum_personal_sign(&mut device, &path, message, ui.as_ref())
                })
                .await
                .map_err(|e| WalletError::SigningFailed(format!("Trezor task: {e}")))??;
                Ok(SignResult::SignatureHex(hex))
            }
            SignRequest::EvmTypedData { .. } | SignRequest::EvmTypedDataHash { .. } => {
                Err(WalletError::HardwareUnsupported(
                    "Trezor EIP-712 clear-signing is not wired yet — use Ledger or a software account"
                        .into(),
                ))
            }
            SignRequest::EvmTransaction { tx } => {
                let raw = tokio::task::spawn_blocking(move || {
                    let mut device = usb::open_initialized(passphrase.as_ref(), ui.as_ref())?;
                    let got = usb::ethereum_address(&mut device, &path, ui.as_ref())?;
                    if !got.eq_ignore_ascii_case(&expected) {
                        return Err(WalletError::HardwareUnsupported(format!(
                            "Trezor address {got} does not match watch record {expected}"
                        )));
                    }
                    usb::ethereum_sign_prepared_tx(&mut device, &path, &tx, ui.as_ref())
                })
                .await
                .map_err(|e| WalletError::SigningFailed(format!("Trezor task: {e}")))??;
                Ok(SignResult::RawTx(raw))
            }
        }
    }
}

/// Preview Live-style paths `0..count` (device unlocked; PIN via `ui` on Trezor One).
pub async fn preview_trezor_live_paths(
    count: usize,
    _chain_id: Option<u64>,
    ui: Option<Arc<TrezorUiBridge>>,
) -> Result<Vec<(String, String)>, WalletError> {
    let paths = evm_live_preview_paths(count);
    tokio::task::spawn_blocking(move || {
        let mut device = usb::open_initialized(None, ui.as_ref())?;
        let mut out = Vec::with_capacity(paths.len());
        for path in paths {
            let addr = usb::ethereum_address(&mut device, &path, ui.as_ref())?;
            out.push((path, addr));
        }
        Ok(out)
    })
    .await
    .map_err(|e| WalletError::SigningFailed(format!("Trezor task: {e}")))?
}

/// Blocking preview for a Keys worker thread (same as async, no runtime required).
pub fn preview_trezor_live_paths_blocking(
    count: usize,
    ui: Option<Arc<TrezorUiBridge>>,
) -> Result<Vec<(String, String)>, WalletError> {
    let paths = evm_live_preview_paths(count);
    let mut device = usb::open_initialized(None, ui.as_ref())?;
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let addr = usb::ethereum_address(&mut device, &path, ui.as_ref())?;
        out.push((path, addr));
    }
    Ok(out)
}

/// Read address at `path`.
pub async fn trezor_address_for_path(
    path: &str,
    _chain_id: Option<u64>,
    ui: Option<Arc<TrezorUiBridge>>,
) -> Result<String, WalletError> {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        let mut device = usb::open_initialized(None, ui.as_ref())?;
        usb::ethereum_address(&mut device, &path, ui.as_ref())
    })
    .await
    .map_err(|e| WalletError::SigningFailed(format!("Trezor task: {e}")))?
}

pub fn trezor_address_for_path_blocking(
    path: &str,
    ui: Option<Arc<TrezorUiBridge>>,
) -> Result<String, WalletError> {
    let mut device = usb::open_initialized(None, ui.as_ref())?;
    usb::ethereum_address(&mut device, path, ui.as_ref())
}

/// Build a watch record after a successful USB address read.
pub async fn discover_trezor_account(
    path: &str,
    chain_id: Option<u64>,
    network_id: Option<String>,
    label: impl Into<String>,
    ui: Option<Arc<TrezorUiBridge>>,
) -> Result<HardwareAccountRecord, WalletError> {
    let address = trezor_address_for_path(path, chain_id, ui).await?;
    Ok(HardwareAccountRecord {
        vendor: HardwareVendor::Trezor,
        family: HwChainFamily::Evm,
        derivation_path: path.trim().to_string(),
        network_id,
        address,
        label: label.into(),
    })
}

pub fn discover_trezor_account_blocking(
    path: &str,
    network_id: Option<String>,
    label: impl Into<String>,
    ui: Option<Arc<TrezorUiBridge>>,
) -> Result<HardwareAccountRecord, WalletError> {
    let address = trezor_address_for_path_blocking(path, ui)?;
    Ok(HardwareAccountRecord {
        vendor: HardwareVendor::Trezor,
        family: HwChainFamily::Evm,
        derivation_path: path.trim().to_string(),
        network_id,
        address,
        label: label.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_trezor_record() {
        let rec = HardwareAccountRecord {
            vendor: HardwareVendor::Ledger,
            family: HwChainFamily::Evm,
            derivation_path: "m/44'/60'/0'/0/0".into(),
            network_id: None,
            address: "0x0000000000000000000000000000000000000001".into(),
            label: String::new(),
        };
        assert!(TrezorSignerBackend::new(rec, Some(943)).is_err());
    }

    #[test]
    fn accepts_trezor_record_shape() {
        let rec = HardwareAccountRecord {
            vendor: HardwareVendor::Trezor,
            family: HwChainFamily::Evm,
            derivation_path: "m/44'/60'/0'/0/0".into(),
            network_id: Some("943".into()),
            address: "0x0000000000000000000000000000000000000001".into(),
            label: "t1".into(),
        };
        let b = TrezorSignerBackend::new(rec, Some(943)).unwrap();
        assert_eq!(b.address(), "0x0000000000000000000000000000000000000001");
    }
}
