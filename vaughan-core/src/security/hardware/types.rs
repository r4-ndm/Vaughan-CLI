//! Hardware account records and family-tagged sign request/result types.
//!
//! Watch-only metadata only — no secrets. Derivation paths are opaque strings
//! so Bitcoin/Polkadot can reuse the same record shape later.

use alloy::primitives::B256;
use serde::{Deserialize, Serialize};

use crate::chains::EvmTransaction;

/// Stable index base for hardware watch accounts (imports stay at `1_000_000`).
pub const HARDWARE_INDEX_BASE: u32 = 2_000_000;

/// Device vendor. Extended when a new HID backend is approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum HardwareVendor {
    Ledger,
    Trezor,
}

impl HardwareVendor {
    /// Short label for F3 / Keys UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ledger => "Ledger",
            Self::Trezor => "Trezor",
        }
    }
}

/// Chain family this watch-record belongs to (mirrors [`crate::chains::ChainType`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum HwChainFamily {
    #[default]
    Evm,
}

impl HwChainFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Evm => "EVM",
        }
    }
}

/// Persisted hardware watch account (address + path; keys stay on device).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareAccountRecord {
    pub vendor: HardwareVendor,
    #[serde(default)]
    pub family: HwChainFamily,
    /// Opaque derivation string (EVM: BIP-44 `m/44'/60'/0'/0/0`).
    pub derivation_path: String,
    /// Optional opaque network hint (EVM chain id string, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    /// Family-validated address; re-verified on connect in Phase 1+.
    pub address: String,
    #[serde(default)]
    pub label: String,
}

impl HardwareAccountRecord {
    /// Default display label when `label` is empty.
    pub fn display_label(&self) -> String {
        let custom = self.label.trim();
        if !custom.is_empty() {
            return custom.to_string();
        }
        format!(
            "{} · {} · {}",
            self.vendor.as_str(),
            self.family.as_str(),
            self.derivation_path
        )
    }
}

/// How an unlocked account was created (HD / imported hex / hardware watch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountKind {
    Hd,
    Imported,
    Hardware(HardwareAccountRecord),
}

impl AccountKind {
    pub fn is_hardware(&self) -> bool {
        matches!(self, Self::Hardware(_))
    }

    pub fn is_imported(&self) -> bool {
        matches!(self, Self::Imported)
    }
}

/// Family-tagged signing request. Core [`super::SignerBackend`] stays multichain.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SignRequest {
    EvmPersonal {
        message: Vec<u8>,
    },
    /// Full EIP-712 JSON (required for Ledger; local may hash then sign).
    EvmTypedData {
        payload: serde_json::Value,
    },
    /// Pre-hashed typed data (local only — Ledger rejects this).
    EvmTypedDataHash {
        hash: B256,
    },
    /// Prepared EIP-1559 (or legacy) fields; nonce/gas/fees must already be set.
    EvmTransaction {
        tx: EvmTransaction,
    },
}

/// Family-tagged signing result.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SignResult {
    /// `0x`-prefixed `r ‖ s ‖ v` hex (personal / typed / hash).
    SignatureHex(String),
    /// EIP-2718 encoded signed tx (no `0x` prefix).
    RawTx(Vec<u8>),
}
