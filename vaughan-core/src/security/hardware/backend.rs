//! Family-agnostic [`SignerBackend`] and the local (software) implementation.

use alloy::signers::local::PrivateKeySigner;
use async_trait::async_trait;

use crate::error::WalletError;
use crate::security::signing::sign_typed_data;

use super::profiles::evm::sign_evm_local;
use super::types::{HwChainFamily, SignRequest, SignResult};

/// Async, family-agnostic signing surface (local today; Ledger/Trezor later).
#[async_trait]
pub trait SignerBackend: Send + Sync {
    fn address(&self) -> &str;
    fn family(&self) -> HwChainFamily;
    async fn sign(&self, req: SignRequest) -> Result<SignResult, WalletError>;
}

/// Software EOA backed by [`PrivateKeySigner`] (EVM variants only in Phase 0).
pub struct LocalSignerBackend {
    address: String,
    signer: PrivateKeySigner,
}

impl LocalSignerBackend {
    /// Wrap an unlocked local key. Caller must drop when done.
    pub fn new(signer: PrivateKeySigner) -> Self {
        let address = signer.address().to_string();
        Self { address, signer }
    }

    /// Borrow the underlying local signer (AA / legacy sync paths that still
    /// require [`PrivateKeySigner`]; hardware backends will not expose this).
    pub fn local_signer(&self) -> &PrivateKeySigner {
        &self.signer
    }

    /// EIP-712 JSON convenience (hashes then signs via [`Self::sign`]-equivalent).
    pub fn sign_typed_data_json(
        &self,
        typed_data: &serde_json::Value,
    ) -> Result<String, WalletError> {
        sign_typed_data(&self.signer, typed_data)
    }
}

#[async_trait]
impl SignerBackend for LocalSignerBackend {
    fn address(&self) -> &str {
        &self.address
    }

    fn family(&self) -> HwChainFamily {
        HwChainFamily::Evm
    }

    async fn sign(&self, req: SignRequest) -> Result<SignResult, WalletError> {
        match &req {
            SignRequest::EvmPersonal { .. }
            | SignRequest::EvmTypedDataHash { .. }
            | SignRequest::EvmTransaction { .. } => sign_evm_local(&self.signer, req).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::hd_wallet::{derive_account, validate_mnemonic};

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[tokio::test]
    async fn local_backend_personal_sign() {
        let signer = derive_account(&validate_mnemonic(TEST_MNEMONIC).unwrap(), 0).unwrap();
        let backend = LocalSignerBackend::new(signer);
        assert_eq!(backend.family(), HwChainFamily::Evm);
        let sig = backend
            .sign(SignRequest::EvmPersonal {
                message: b"vaughan".to_vec(),
            })
            .await
            .unwrap();
        assert!(matches!(sig, SignResult::SignatureHex(_)));
    }
}
