//! Chain-agnostic value types shared by all chain adapters.
//!
//! The types here are deliberately family-neutral. Family-specific data lives
//! in per-family modules (`chains/evm`, `chains/bitcoin`, …) and is carried
//! through the tagged enums [`ChainTransaction`] and [`FeeDetails`].

use std::fmt;
use std::str::FromStr;

use alloy::primitives::utils::format_units;
use alloy::primitives::U256;
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

/// User-facing EIP-1559 speed preset (wallet UX, not an on-chain enum).
///
/// Base fees come from Alloy's `estimate_eip1559_fees` (feeHistory percentiles,
/// same family of algorithm MetaMask/ethers use). Presets only scale that
/// suggestion — we do not pull Ambire or other wallet source for this.
/// [`FeeSpeed::Custom`] skips scaling; the TUI supplies an explicit max fee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeeSpeed {
    /// Lower tip / headroom — may wait longer in the mempool.
    Slow,
    /// Unmodified Alloy / network suggestion.
    #[default]
    Normal,
    /// Higher tip for quicker inclusion.
    Fast,
    /// Aggressive tip for congested moments ("ape").
    Ape,
    /// User-entered max fee (gwei); not a scale of the Alloy suggestion.
    Custom,
}

impl FeeSpeed {
    /// Digits `1`–`5` / ↑↓ cycle order.
    pub const ALL: [Self; 5] = [
        Self::Slow,
        Self::Normal,
        Self::Fast,
        Self::Ape,
        Self::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Slow => "Slow",
            Self::Normal => "Normal",
            Self::Fast => "Fast",
            Self::Ape => "Ape",
            Self::Custom => "Custom",
        }
    }

    /// `1` Slow · `2` Normal · `3` Fast · `4` Ape · `5` Custom.
    pub fn from_digit(c: char) -> Option<Self> {
        match c {
            '1' => Some(Self::Slow),
            '2' => Some(Self::Normal),
            '3' => Some(Self::Fast),
            '4' => Some(Self::Ape),
            '5' => Some(Self::Custom),
            _ => None,
        }
    }

    /// Next preset in [`Self::ALL`] (wraps).
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&s| s == self).unwrap_or(1);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// Previous preset in [`Self::ALL`] (wraps).
    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|&s| s == self).unwrap_or(1);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    /// `(max_fee_bps, priority_fee_bps)` relative to the base estimate (10_000 = 100%).
    ///
    /// `None` for [`Self::Custom`] — callers must set an absolute max fee.
    fn scale_bps(self) -> Option<(u32, u32)> {
        Some(match self {
            Self::Slow => (9_000, 7_000),
            Self::Normal => (10_000, 10_000),
            Self::Fast => (12_500, 15_000),
            Self::Ape => (20_000, 25_000),
            Self::Custom => return None,
        })
    }
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

impl Fee {
    /// Scale EIP-1559 max/priority fees for a speed preset; gas limit unchanged.
    ///
    /// [`FeeSpeed::Custom`] returns `self` unchanged — use
    /// [`Self::with_custom_max_fee_gwei`] for an absolute max fee.
    /// Non-EVM fee details are returned unchanged.
    pub fn with_speed(&self, speed: FeeSpeed) -> Self {
        let FeeDetails::Evm {
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
        } = &self.details
        else {
            return self.clone();
        };

        let Some((max_bps, tip_bps)) = speed.scale_bps() else {
            return self.clone();
        };
        let scale = |raw: &Option<String>, bps: u32| -> Option<String> {
            let s = raw.as_deref()?;
            let v = U256::from_str(s).ok()?;
            Some((v.saturating_mul(U256::from(bps)) / U256::from(10_000u32)).to_string())
        };

        let mut new_max = scale(max_fee_per_gas, max_bps);
        let new_tip = scale(max_priority_fee_per_gas, tip_bps);
        if let (Some(max_s), Some(tip_s)) = (&new_max, &new_tip) {
            if let (Ok(max_v), Ok(tip_v)) = (U256::from_str(max_s), U256::from_str(tip_s)) {
                if max_v < tip_v {
                    new_max = Some(tip_s.clone());
                }
            }
        }

        self.with_evm_fees(*gas_limit, new_max, new_tip)
    }

    /// Replace max fee with an absolute gwei value; tip stays the base tip
    /// (clamped so tip ≤ max). Gas limit unchanged.
    pub fn with_custom_max_fee_gwei(&self, gwei: &str) -> Result<Self, String> {
        let FeeDetails::Evm {
            gas_limit,
            max_priority_fee_per_gas,
            ..
        } = &self.details
        else {
            return Err("custom gas is only supported for EVM fees".into());
        };
        let trimmed = gwei.trim();
        if trimmed.is_empty() {
            return Err("enter a max fee in gwei".into());
        }
        let units = alloy::primitives::utils::parse_units(trimmed, 9)
            .map_err(|_| format!("invalid gwei amount: {trimmed}"))?;
        if units.is_negative() {
            return Err(format!("gwei must be positive: {trimmed}"));
        }
        let max_wei: U256 = units.into();
        if max_wei.is_zero() {
            return Err("max fee must be greater than zero".into());
        }
        let tip = max_priority_fee_per_gas
            .as_deref()
            .and_then(|s| U256::from_str(s).ok())
            .unwrap_or_default()
            .min(max_wei);
        Ok(self.with_evm_fees(*gas_limit, Some(max_wei.to_string()), Some(tip.to_string())))
    }

    fn with_evm_fees(
        &self,
        gas_limit: u64,
        max_fee_per_gas: Option<String>,
        max_priority_fee_per_gas: Option<String>,
    ) -> Self {
        let per_gas = max_fee_per_gas
            .as_deref()
            .and_then(|s| U256::from_str(s).ok())
            .unwrap_or_default();
        let total_wei = per_gas.saturating_mul(U256::from(gas_limit));
        // Display uses 18 decimals as a safe default for native EVM amounts;
        // the currency symbol is preserved from the base estimate.
        let total_formatted = format_units(total_wei, 18).unwrap_or_else(|_| "0.0".to_string());
        let total = format!("{total_formatted} {}", self.currency);

        Self {
            total,
            currency: self.currency.clone(),
            details: FeeDetails::Evm {
                gas_limit,
                max_fee_per_gas,
                max_priority_fee_per_gas,
            },
        }
    }

    /// Upper-bound native cost in wei: `max_fee_per_gas * gas_limit` (EIP-1559).
    ///
    /// Used by MCP fee-spike checks and agent proposal stamping; matches the
    /// wallet approval path in `vaughan-tui::provider`.
    pub fn total_wei_evm(&self) -> Option<U256> {
        let FeeDetails::Evm {
            gas_limit,
            max_fee_per_gas,
            ..
        } = &self.details
        else {
            return None;
        };
        let max_s = max_fee_per_gas.as_deref()?;
        let per_gas = U256::from_str(max_s).ok()?;
        Some(per_gas.saturating_mul(U256::from(*gas_limit)))
    }
}

/// Family-specific fee parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Non-zero ERC-20 allowance for the Approvals manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowanceEntry {
    pub token: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub spender: String,
    pub spender_label: String,
    /// Raw allowance in base units.
    pub amount: String,
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

    #[test]
    fn fee_total_wei_evm() {
        let fee = Fee {
            total: "0.000063 ETH".into(),
            currency: "ETH".into(),
            details: FeeDetails::Evm {
                gas_limit: 21_000,
                max_fee_per_gas: Some("2000000000".into()),
                max_priority_fee_per_gas: Some("1000000000".into()),
            },
        };
        assert_eq!(
            fee.total_wei_evm(),
            Some(U256::from(21_000u64) * U256::from(2_000_000_000u64))
        );
        let legacy = Fee {
            total: "0".into(),
            currency: "BTC".into(),
            details: FeeDetails::Bitcoin {
                fee_rate_sat_per_vbyte: "1".into(),
                estimated_vsize: 140,
            },
        };
        assert!(legacy.total_wei_evm().is_none());
    }

    #[test]
    fn fee_speed_scales_eip1559_and_keeps_max_gte_tip() {
        let base = Fee {
            total: "0.000063 ETH".into(),
            currency: "ETH".into(),
            details: FeeDetails::Evm {
                gas_limit: 21_000,
                max_fee_per_gas: Some("2000000000".into()), // 2 gwei
                max_priority_fee_per_gas: Some("1000000000".into()), // 1 gwei
            },
        };
        let ape = base.with_speed(FeeSpeed::Ape);
        let FeeDetails::Evm {
            max_fee_per_gas,
            max_priority_fee_per_gas,
            ..
        } = &ape.details
        else {
            panic!("expected EVM fee");
        };
        let max = U256::from_str(max_fee_per_gas.as_deref().unwrap()).unwrap();
        let tip = U256::from_str(max_priority_fee_per_gas.as_deref().unwrap()).unwrap();
        assert_eq!(max, U256::from(4_000_000_000u64)); // 200% of 2 gwei
        assert_eq!(tip, U256::from(2_500_000_000u64)); // 250% of 1 gwei
        assert!(max >= tip);

        let normal = base.with_speed(FeeSpeed::Normal);
        assert_eq!(
            normal.details,
            FeeDetails::Evm {
                gas_limit: 21_000,
                max_fee_per_gas: Some("2000000000".into()),
                max_priority_fee_per_gas: Some("1000000000".into()),
            }
        );
        assert_eq!(FeeSpeed::from_digit('3'), Some(FeeSpeed::Fast));
        assert_eq!(FeeSpeed::from_digit('5'), Some(FeeSpeed::Custom));
        assert_eq!(FeeSpeed::Normal.next(), FeeSpeed::Fast);
        assert_eq!(FeeSpeed::Custom.next(), FeeSpeed::Slow);
        assert_eq!(FeeSpeed::Slow.prev(), FeeSpeed::Custom);
        assert_eq!(base.with_speed(FeeSpeed::Custom).details, base.details);

        let custom = base.with_custom_max_fee_gwei("50").unwrap();
        let FeeDetails::Evm {
            max_fee_per_gas,
            max_priority_fee_per_gas,
            ..
        } = &custom.details
        else {
            panic!("expected EVM fee");
        };
        assert_eq!(max_fee_per_gas.as_deref(), Some("50000000000"));
        // Base tip 1 gwei stays (below custom max).
        assert_eq!(max_priority_fee_per_gas.as_deref(), Some("1000000000"));
        assert!(base.with_custom_max_fee_gwei("").is_err());
        assert!(base.with_custom_max_fee_gwei("0").is_err());
    }
}
