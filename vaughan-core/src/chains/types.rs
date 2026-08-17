//! Chain-agnostic value types shared by all chain adapters.
//!
//! The types here are deliberately family-neutral. Family-specific data lives
//! in per-family modules (`chains/evm`, `chains/bitcoin`, …) and is carried
//! through the tagged enums [`ChainTransaction`] and [`FeeDetails`].

use std::fmt;

use serde::{Deserialize, Serialize};

/// Supported blockchain families.
///
/// EVM is the first-class target. `Bitcoin` and `Polkadot` are reserved for
/// future phases; adding a family means adding a variant here plus a module
/// under `chains/`. Kept `#[non_exhaustive]` so external matchers never break
/// when a family is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ChainType {
    Evm,
    Bitcoin,
    Polkadot,
}

impl fmt::Display for ChainType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evm => write!(f, "EVM"),
            Self::Bitcoin => write!(f, "Bitcoin"),
            Self::Polkadot => write!(f, "Polkadot"),
        }
    }
}

/// Token information (native asset or ERC-20).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    pub contract_address: Option<String>,
}

/// A balance in raw units plus a human-readable formatted value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    pub token: TokenInfo,
    pub raw: String,
    pub formatted: String,
    pub usd_value: Option<f64>,
}

/// Chain info (family, opaque network id, name, rpc url).
///
/// `network_id` is family-specific: an EVM chain id (decimal string),
/// Bitcoin "mainnet"/"testnet", or a Polkadot genesis hash / SS58 prefix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainInfo {
    pub chain_type: ChainType,
    pub network_id: String,
    pub name: String,
    pub rpc_url: String,
}

/// A transaction id/hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxHash(pub String);

impl fmt::Display for TxHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TxHash {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// A chain-agnostic transaction request, tagged by family.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "chain_type")]
#[non_exhaustive]
pub enum ChainTransaction {
    Evm(EvmTransaction),
    Bitcoin(BitcoinTransaction),
    Polkadot(PolkadotTransaction),
}

/// EVM transaction parameters. Monetary values are decimal strings in wei.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTransaction {
    pub from: String,
    pub to: String,
    pub value: String,
    pub data: Option<String>,
    pub gas_limit: Option<u64>,
    pub gas_price: Option<String>,
    pub max_fee_per_gas: Option<String>,
    pub max_priority_fee_per_gas: Option<String>,
    pub nonce: Option<u64>,
    pub chain_id: u64,
}

/// Bitcoin (UTXO) transaction request. Monetary values in satoshis.
///
/// Reserved for a future phase; the adapter performs coin selection unless
/// explicit inputs are supplied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitcoinTransaction {
    pub from: String,
    pub to: String,
    pub amount_sats: String,
    pub inputs: Vec<String>,
    pub change_address: Option<String>,
    pub fee_rate_sat_per_vbyte: Option<String>,
}

/// Polkadot/Substrate extrinsic request. Monetary values in the chain's
/// smallest unit (planck).
///
/// Reserved for a future phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolkadotTransaction {
    pub from: String,
    pub to: String,
    pub amount_planck: String,
    pub tip_planck: Option<String>,
    pub nonce: Option<u64>,
}

/// A fee estimate: a display total plus family-specific details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fee {
    /// Human-readable total, e.g. "0.00021 ETH".
    pub total: String,
    /// Native token symbol, e.g. "ETH".
    pub currency: String,
    /// Family-specific breakdown.
    pub details: FeeDetails,
}

/// Family-specific fee parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "chain_type")]
#[non_exhaustive]
pub enum FeeDetails {
    /// EIP-1559 / legacy gas parameters.
    Evm {
        gas_limit: u64,
        max_fee_per_gas: Option<String>,
        max_priority_fee_per_gas: Option<String>,
    },
    /// Bitcoin fee rate and estimated virtual size.
    Bitcoin {
        fee_rate_sat_per_vbyte: String,
        estimated_vsize: u64,
    },
    /// Substrate weight + tip.
    Polkadot {
        weight: String,
        partial_fee_planck: String,
        tip_planck: Option<String>,
    },
}

/// A transaction record for history views.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxRecord {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub value: String,
    pub status: TxStatus,
    pub block_number: Option<u64>,
    pub timestamp: Option<u64>,
    pub gas_used: Option<u64>,
    pub token_symbol: Option<String>,
    pub token_address: Option<String>,
    pub is_token_transfer: bool,
}

/// Transaction status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxStatus {
    Pending,
    Confirmed,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_type_display() {
        assert_eq!(ChainType::Evm.to_string(), "EVM");
        assert_eq!(ChainType::Bitcoin.to_string(), "Bitcoin");
        assert_eq!(ChainType::Polkadot.to_string(), "Polkadot");
    }

    #[test]
    fn evm_transaction_serde_roundtrip() {
        let tx = ChainTransaction::Evm(EvmTransaction {
            from: "0xabc".into(),
            to: "0xdef".into(),
            value: "1000000000000000000".into(),
            data: Some("0x".into()),
            gas_limit: Some(21_000),
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: Some(0),
            chain_id: 369,
        });
        let json = serde_json::to_string(&tx).unwrap();
        let back: ChainTransaction = serde_json::from_str(&json).unwrap();
        match back {
            ChainTransaction::Evm(e) => assert_eq!(e.chain_id, 369),
            _ => panic!("expected Evm variant"),
        }
    }

    #[test]
    fn fee_details_serde_roundtrip() {
        let fee = Fee {
            total: "0.001 PLS".into(),
            currency: "PLS".into(),
            details: FeeDetails::Evm {
                gas_limit: 21_000,
                max_fee_per_gas: Some("3000000000".into()),
                max_priority_fee_per_gas: Some("1500000000".into()),
            },
        };
        let json = serde_json::to_string(&fee).unwrap();
        let back: Fee = serde_json::from_str(&json).unwrap();
        assert_eq!(back.currency, "PLS");
    }
}
