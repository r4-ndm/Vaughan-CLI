//! CI / Anvil mock hardware signer (no USB).
//!
//! Lets integration tests exercise the hardware account path with a local
//! [`PrivateKeySigner`] that pretends to be a Ledger watch record.

use alloy::signers::local::PrivateKeySigner;
use async_trait::async_trait;

use crate::error::WalletError;

use super::backend::SignerBackend;
use super::profiles::evm::sign_evm_local;
use super::session::DeviceSession;
use super::types::{HardwareAccountRecord, HardwareVendor, HwChainFamily, SignRequest, SignResult};

/// Local key that implements [`SignerBackend`] for a hardware watch address.
#[derive(Clone)]
pub struct MockSignerBackend {
    address: String,
    signer: PrivateKeySigner,
}

impl MockSignerBackend {
    /// Build a mock whose address must match the hardware watch record in tests.
    pub fn new(signer: PrivateKeySigner) -> Self {
        let address = signer.address().to_string();
        Self { address, signer }
    }

    pub fn address_string(&self) -> &str {
        &self.address
    }

    /// Watch record pointing at this mock (Ledger vendor label for UX parity).
    pub fn watch_record(&self, path: &str, network_id: Option<String>) -> HardwareAccountRecord {
        HardwareAccountRecord {
            vendor: HardwareVendor::Ledger,
            family: HwChainFamily::Evm,
            derivation_path: path.to_string(),
            network_id,
            address: self.address.clone(),
            label: "Mock Ledger".into(),
        }
    }
}

#[async_trait]
impl SignerBackend for MockSignerBackend {
    fn address(&self) -> &str {
        &self.address
    }

    fn family(&self) -> HwChainFamily {
        HwChainFamily::Evm
    }

    async fn sign(&self, req: SignRequest) -> Result<SignResult, WalletError> {
        match req {
            SignRequest::EvmTypedData { payload } => {
                let hex = crate::security::signing::sign_typed_data(&self.signer, &payload)?;
                Ok(SignResult::SignatureHex(hex))
            }
            other => sign_evm_local(&self.signer, other).await,
        }
    }
}

/// [`DeviceSession`] that returns a single fixed path/address (no HID).
pub struct MockDeviceSession {
    path: String,
    address: String,
}

impl MockDeviceSession {
    pub fn new(path: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            address: address.into(),
        }
    }
}

#[async_trait]
impl DeviceSession for MockDeviceSession {
    fn vendor(&self) -> HardwareVendor {
        HardwareVendor::Ledger
    }

    async fn list_paths_preview(
        &self,
        family: HwChainFamily,
    ) -> Result<Vec<(String, String)>, WalletError> {
        if !matches!(family, HwChainFamily::Evm) {
            return Err(WalletError::HardwareUnsupported(
                "mock device is EVM-only".into(),
            ));
        }
        Ok(vec![(self.path.clone(), self.address.clone())])
    }

    async fn address_for_path(
        &self,
        family: HwChainFamily,
        path: &str,
    ) -> Result<String, WalletError> {
        if !matches!(family, HwChainFamily::Evm) {
            return Err(WalletError::HardwareUnsupported(
                "mock device is EVM-only".into(),
            ));
        }
        if path != self.path {
            return Err(WalletError::AccountNotFound(format!(
                "mock path {path} not found"
            )));
        }
        Ok(self.address.clone())
    }

    async fn sign_preimage(
        &self,
        _family: HwChainFamily,
        _path: &str,
        _preimage: &[u8],
    ) -> Result<Vec<u8>, WalletError> {
        Err(WalletError::HardwareUnsupported(
            "mock device signs via MockSignerBackend, not raw preimage".into(),
        ))
    }
}
