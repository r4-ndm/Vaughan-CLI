//! Ledger USB [`DeviceSession`] and [`LedgerSignerBackend`].
//!
//! Transport + path/address only in the session; EVM signing uses Alloy's
//! [`LedgerSigner`] (confirm-on-device). No fee/RPC here.

use std::str::FromStr;
use std::sync::Arc;

use alloy::eips::eip2718::Encodable2718;
use alloy::network::{EthereumWallet, NetworkTransactionBuilder};
use alloy::primitives::{Address, TxKind, U256};
use alloy::rpc::types::eth::TransactionRequest;
use alloy::signers::Signer;
use alloy_signer_ledger::{
    coins_ledger::transports::{Ledger, LedgerAsync},
    HDPath, LedgerSigner,
};
use async_trait::async_trait;
use futures_util::lock::Mutex;

use crate::chains::EvmTransaction;
use crate::error::WalletError;

use super::backend::SignerBackend;
use super::session::DeviceSession;
use super::types::{HardwareAccountRecord, HardwareVendor, HwChainFamily, SignRequest, SignResult};

fn map_ledger(err: impl std::fmt::Display) -> WalletError {
    let msg = err.to_string();
    let lower = msg.to_lowercase();
    if lower.contains("hid") || lower.contains("device") || lower.contains("not found") {
        WalletError::HardwareUnsupported(format!(
            "Ledger not ready — unlock, open the Ethereum app, check USB/udev: {msg}"
        ))
    } else if lower.contains("reject") || lower.contains("denied") {
        WalletError::SigningFailed("rejected on Ledger".into())
    } else {
        WalletError::SigningFailed(format!("Ledger: {msg}"))
    }
}

/// Parse a stored opaque path into Alloy's [`HDPath`].
pub fn hd_path_from_str(path: &str) -> Result<HDPath, WalletError> {
    let p = path.trim();
    if p.is_empty() {
        return Err(WalletError::InvalidTransaction(
            "empty Ledger derivation path".into(),
        ));
    }
    if let Some(rest) = p.strip_prefix("m/44'/60'/") {
        if let Some(idx) = rest.strip_suffix("'/0/0") {
            if let Ok(n) = idx.parse::<usize>() {
                return Ok(HDPath::LedgerLive(n));
            }
        }
        if let Some(idx) = rest.strip_prefix("0'/") {
            if let Ok(n) = idx.parse::<usize>() {
                return Ok(HDPath::Legacy(n));
            }
        }
    }
    Ok(HDPath::Other(p.to_string()))
}

/// Open Ledger once and preview Ledger Live paths `0..count`.
pub async fn preview_ledger_live_paths(
    count: usize,
    chain_id: Option<u64>,
) -> Result<Vec<(String, String)>, WalletError> {
    let transport = Arc::new(Mutex::new(Ledger::init().await.map_err(map_ledger)?));
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let derivation = HDPath::LedgerLive(i);
        let signer =
            LedgerSigner::new_with_transport(derivation.clone(), chain_id, transport.clone())
                .await
                .map_err(map_ledger)?;
        out.push((derivation.to_string(), signer.address().to_string()));
    }
    Ok(out)
}

/// Connect and read the address for `path` (Ethereum app must be open).
pub async fn ledger_address_for_path(
    path: &str,
    chain_id: Option<u64>,
) -> Result<String, WalletError> {
    let derivation = hd_path_from_str(path)?;
    let signer = LedgerSigner::new(derivation, chain_id)
        .await
        .map_err(map_ledger)?;
    Ok(signer.address().to_string())
}

/// Build a watch record after connecting (caller should confirm address on device UX).
pub async fn discover_ledger_account(
    path: &str,
    chain_id: Option<u64>,
    network_id: Option<String>,
    label: impl Into<String>,
) -> Result<HardwareAccountRecord, WalletError> {
    let address = ledger_address_for_path(path, chain_id).await?;
    Ok(HardwareAccountRecord {
        vendor: HardwareVendor::Ledger,
        family: HwChainFamily::Evm,
        derivation_path: path.trim().to_string(),
        network_id,
        address,
        label: label.into(),
    })
}

/// [`DeviceSession`] over a shared Ledger HID transport (discovery / path probe).
pub struct LedgerDeviceSession {
    transport: Arc<Mutex<Ledger>>,
    chain_id: Option<u64>,
}

impl LedgerDeviceSession {
    /// Open the first compatible Ledger (Ethereum app must be running).
    pub async fn connect(chain_id: Option<u64>) -> Result<Self, WalletError> {
        let transport = Ledger::init().await.map_err(map_ledger)?;
        Ok(Self {
            transport: Arc::new(Mutex::new(transport)),
            chain_id,
        })
    }
}

#[async_trait]
impl DeviceSession for LedgerDeviceSession {
    fn vendor(&self) -> HardwareVendor {
        HardwareVendor::Ledger
    }

    async fn list_paths_preview(
        &self,
        family: HwChainFamily,
    ) -> Result<Vec<(String, String)>, WalletError> {
        if !matches!(family, HwChainFamily::Evm) {
            return Err(WalletError::HardwareUnsupported(
                "Ledger Phase 1 only supports EVM paths".into(),
            ));
        }
        let mut out = Vec::with_capacity(5);
        for i in 0..5 {
            let derivation = HDPath::LedgerLive(i);
            let signer = LedgerSigner::new_with_transport(
                derivation.clone(),
                self.chain_id,
                self.transport.clone(),
            )
            .await
            .map_err(map_ledger)?;
            out.push((derivation.to_string(), signer.address().to_string()));
        }
        Ok(out)
    }

    async fn address_for_path(
        &self,
        family: HwChainFamily,
        path: &str,
    ) -> Result<String, WalletError> {
        if !matches!(family, HwChainFamily::Evm) {
            return Err(WalletError::HardwareUnsupported(
                "Ledger Phase 1 only supports EVM paths".into(),
            ));
        }
        let derivation = hd_path_from_str(path)?;
        let signer =
            LedgerSigner::new_with_transport(derivation, self.chain_id, self.transport.clone())
                .await
                .map_err(map_ledger)?;
        Ok(signer.address().to_string())
    }

    async fn sign_preimage(
        &self,
        _family: HwChainFamily,
        _path: &str,
        _preimage: &[u8],
    ) -> Result<Vec<u8>, WalletError> {
        Err(WalletError::HardwareUnsupported(
            "Ledger does not support raw preimage signing — use LedgerSignerBackend".into(),
        ))
    }
}

/// EVM [`SignerBackend`] that opens the Ledger for each sign (confirm on device).
pub struct LedgerSignerBackend {
    record: HardwareAccountRecord,
    chain_id: Option<u64>,
}

impl LedgerSignerBackend {
    pub fn new(record: HardwareAccountRecord, chain_id: Option<u64>) -> Result<Self, WalletError> {
        if record.vendor != HardwareVendor::Ledger {
            return Err(WalletError::HardwareUnsupported(
                "expected a Ledger watch account".into(),
            ));
        }
        if !matches!(record.family, HwChainFamily::Evm) {
            return Err(WalletError::HardwareUnsupported(
                "Ledger Phase 1 is EVM-only".into(),
            ));
        }
        Ok(Self { record, chain_id })
    }

    async fn connect(&self) -> Result<LedgerSigner, WalletError> {
        let derivation = hd_path_from_str(&self.record.derivation_path)?;
        let signer = LedgerSigner::new(derivation, self.chain_id)
            .await
            .map_err(map_ledger)?;
        let got = signer.address().to_string();
        if !got.eq_ignore_ascii_case(&self.record.address) {
            return Err(WalletError::HardwareUnsupported(format!(
                "Ledger address {got} does not match watch record {}",
                self.record.address
            )));
        }
        Ok(signer)
    }
}

#[async_trait]
impl SignerBackend for LedgerSignerBackend {
    fn address(&self) -> &str {
        &self.record.address
    }

    fn family(&self) -> HwChainFamily {
        HwChainFamily::Evm
    }

    async fn sign(&self, req: SignRequest) -> Result<SignResult, WalletError> {
        let ledger = self.connect().await?;
        match req {
            SignRequest::EvmPersonal { message } => {
                let sig = ledger.sign_message(&message).await.map_err(map_ledger)?;
                Ok(SignResult::SignatureHex(format!(
                    "0x{}",
                    hex::encode(sig.as_bytes())
                )))
            }
            SignRequest::EvmTypedData { payload } => {
                let typed: alloy_dyn_abi::TypedData =
                    serde_json::from_value(payload).map_err(|e| {
                        WalletError::InvalidTransaction(format!("invalid EIP-712 typed data: {e}"))
                    })?;
                let sig = ledger
                    .sign_dynamic_typed_data(&typed)
                    .await
                    .map_err(map_ledger)?;
                Ok(SignResult::SignatureHex(format!(
                    "0x{}",
                    hex::encode(sig.as_bytes())
                )))
            }
            SignRequest::EvmTypedDataHash { .. } => Err(WalletError::HardwareUnsupported(
                "Ledger cannot sign a bare typed-data hash — pass full EIP-712 JSON".into(),
            )),
            SignRequest::EvmTransaction { tx } => {
                let raw = sign_prepared_evm_tx_ledger(ledger, &tx).await?;
                Ok(SignResult::RawTx(raw))
            }
        }
    }
}

async fn sign_prepared_evm_tx_ledger(
    ledger: LedgerSigner,
    evm_tx: &EvmTransaction,
) -> Result<Vec<u8>, WalletError> {
    if evm_tx.nonce.is_none() {
        return Err(WalletError::InvalidTransaction(
            "nonce required before Ledger envelope sign".into(),
        ));
    }
    let from = Address::from_str(evm_tx.from.trim()).map_err(|_| {
        WalletError::InvalidTransaction(format!("invalid address: {}", evm_tx.from))
    })?;
    if from != ledger.address() {
        return Err(WalletError::SigningFailed(
            "transaction from does not match Ledger address".into(),
        ));
    }
    let to = Address::from_str(evm_tx.to.trim())
        .map_err(|_| WalletError::InvalidTransaction(format!("invalid address: {}", evm_tx.to)))?;
    let value = U256::from_str(&evm_tx.value)
        .map_err(|_| WalletError::InvalidAmount(format!("Invalid wei value: {}", evm_tx.value)))?;
    let is_create = to.is_zero()
        && evm_tx
            .data
            .as_deref()
            .is_some_and(|d| !d.trim_start_matches("0x").is_empty());
    let mut req = TransactionRequest {
        from: Some(from),
        to: Some(if is_create {
            TxKind::Create
        } else {
            TxKind::Call(to)
        }),
        value: Some(value),
        chain_id: Some(evm_tx.chain_id),
        nonce: evm_tx.nonce,
        gas: evm_tx.gas_limit,
        ..Default::default()
    };
    if evm_tx.max_fee_per_gas.is_none() {
        if let Some(gas_price) = evm_tx.gas_price.as_deref() {
            let gp = U256::from_str(gas_price).map_err(|_| {
                WalletError::InvalidAmount(format!("Invalid gas price: {gas_price}"))
            })?;
            req.gas_price = Some(gp.to::<u128>());
        }
    }
    if let Some(max_fee) = evm_tx.max_fee_per_gas.as_deref() {
        let mf = U256::from_str(max_fee)
            .map_err(|_| WalletError::InvalidAmount(format!("Invalid max fee: {max_fee}")))?;
        req.max_fee_per_gas = Some(mf.to::<u128>());
    }
    if let Some(prio) = evm_tx.max_priority_fee_per_gas.as_deref() {
        let p = U256::from_str(prio)
            .map_err(|_| WalletError::InvalidAmount(format!("Invalid priority fee: {prio}")))?;
        req.max_priority_fee_per_gas = Some(p.to::<u128>());
    }
    if let Some(data_hex) = evm_tx.data.as_deref() {
        let input_bytes = hex::decode(data_hex.trim_start_matches("0x"))
            .map_err(|_| WalletError::InvalidTransaction("Invalid hex data".to_string()))?;
        req.input.input = Some(input_bytes.into());
    }

    // Alloy EthereumWallet over Ledger TxSigner (confirm-on-device).
    let wallet = EthereumWallet::from(ledger);
    let envelope = req
        .build(&wallet)
        .await
        .map_err(|e| WalletError::SigningFailed(e.to_string()))?;
    Ok(envelope.encoded_2718())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ledger_live_and_legacy_paths() {
        match hd_path_from_str("m/44'/60'/0'/0/0").unwrap() {
            HDPath::LedgerLive(0) => {}
            other => panic!("unexpected {other}"),
        }
        match hd_path_from_str("m/44'/60'/0'/3").unwrap() {
            HDPath::Legacy(3) => {}
            other => panic!("unexpected {other}"),
        }
        match hd_path_from_str("m/44'/60'/0'/0/5").unwrap() {
            HDPath::Other(s) => assert!(s.contains("0'/0/5")),
            other => panic!("unexpected {other}"),
        }
    }
}
