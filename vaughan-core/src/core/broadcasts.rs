//! Session-scoped recent broadcasts for History + cancel / speed-up.
//!
//! Entries capture the signed fee/nonce so a pending tx can be replaced
//! (same nonce, higher EIP-1559 fees) without an explorer.

use alloy::primitives::U256;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::chains::{EvmTransaction, TxStatus};
use crate::error::WalletError;

/// Max recent broadcasts kept in the TUI session (newest first).
pub const MAX_RECENT_BROADCASTS: usize = 32;

/// How to replace a pending broadcast (same nonce).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceKind {
    /// Zero-value self-send — drops the original if miners take the replacement.
    Cancel,
    /// Re-submit the same payload with bumped fees.
    SpeedUp,
}

impl ReplaceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cancel => "Cancel",
            Self::SpeedUp => "Speed-up",
        }
    }
}

/// One wallet-originated broadcast (session memory; not persisted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastEntry {
    pub hash: String,
    pub label: String,
    pub chain_id: u64,
    pub from: String,
    pub to: String,
    pub value: String,
    pub data: Option<String>,
    pub nonce: u64,
    pub gas_limit: Option<u64>,
    pub max_fee_per_gas: Option<String>,
    pub max_priority_fee_per_gas: Option<String>,
    pub status: TxStatus,
    /// If this entry replaced another, the prior hash.
    pub replaces: Option<String>,
}

impl BroadcastEntry {
    /// Build from a fully prepared EVM tx (nonce + fees filled) and its hash.
    pub fn from_prepared(tx: &EvmTransaction, hash: String, label: impl Into<String>) -> Self {
        Self {
            hash,
            label: label.into(),
            chain_id: tx.chain_id,
            from: tx.from.clone(),
            to: tx.to.clone(),
            value: tx.value.clone(),
            data: tx.data.clone(),
            nonce: tx.nonce.unwrap_or(0),
            gas_limit: tx.gas_limit,
            max_fee_per_gas: tx.max_fee_per_gas.clone(),
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas.clone(),
            status: TxStatus::Pending,
            replaces: None,
        }
    }

    /// True when cancel / speed-up may still compete in the mempool.
    pub fn is_replaceable(&self) -> bool {
        self.status == TxStatus::Pending
    }

    /// Build a replacement EVM tx (same nonce; bumped fees).
    pub fn replacement_tx(&self, kind: ReplaceKind) -> Result<EvmTransaction, WalletError> {
        let (max_fee, tip) = bump_replacement_fees(
            self.max_fee_per_gas.as_deref(),
            self.max_priority_fee_per_gas.as_deref(),
            kind,
        )?;
        let (to, value, data) = match kind {
            ReplaceKind::Cancel => (self.from.clone(), "0".into(), None),
            ReplaceKind::SpeedUp => (self.to.clone(), self.value.clone(), self.data.clone()),
        };
        Ok(EvmTransaction {
            from: self.from.clone(),
            to,
            value,
            data,
            gas_limit: self.gas_limit.or(Some(21_000)),
            gas_price: None,
            max_fee_per_gas: Some(max_fee),
            max_priority_fee_per_gas: Some(tip),
            nonce: Some(self.nonce),
            chain_id: self.chain_id,
        })
    }
}

/// Hash + bookkeeping entry returned after a tracked broadcast.
#[derive(Debug, Clone)]
pub struct BroadcastReceipt {
    pub hash: String,
    pub entry: BroadcastEntry,
}

impl std::fmt::Display for BroadcastReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hash)
    }
}

/// Bump EIP-1559 fees enough to replace a pending tx (~25% cancel, ~12.5% speed-up).
fn bump_replacement_fees(
    max_fee: Option<&str>,
    tip: Option<&str>,
    kind: ReplaceKind,
) -> Result<(String, String), WalletError> {
    let bps: u32 = match kind {
        ReplaceKind::Cancel => 15_000,  // +50%
        ReplaceKind::SpeedUp => 12_500, // +25%
    };
    let scale = |raw: &str| -> Result<String, WalletError> {
        let v = U256::from_str(raw.trim())
            .map_err(|_| WalletError::InvalidAmount(format!("bad fee: {raw}")))?;
        let bumped = v.saturating_mul(U256::from(bps)) / U256::from(10_000u32);
        let bumped = if bumped <= v {
            v.saturating_add(U256::from(1u64))
        } else {
            bumped
        };
        Ok(bumped.to_string())
    };
    let max = max_fee.ok_or_else(|| {
        WalletError::InvalidTransaction("cannot replace: missing maxFeePerGas".into())
    })?;
    let tip = tip.ok_or_else(|| {
        WalletError::InvalidTransaction("cannot replace: missing maxPriorityFeePerGas".into())
    })?;
    Ok((scale(max)?, scale(tip)?))
}

/// Push `entry` to the front of `list`, capping length.
pub fn push_recent(list: &mut Vec<BroadcastEntry>, entry: BroadcastEntry) {
    list.retain(|e| e.hash != entry.hash);
    list.insert(0, entry);
    if list.len() > MAX_RECENT_BROADCASTS {
        list.truncate(MAX_RECENT_BROADCASTS);
    }
}

/// Mark an older entry superseded when a replacement lands.
pub fn mark_replaced(list: &mut [BroadcastEntry], old_hash: &str, new_hash: &str) {
    for e in list.iter_mut() {
        if e.hash == old_hash {
            e.status = TxStatus::Failed; // superseded / dropped
            e.replaces = None;
        }
        if e.hash == new_hash {
            e.replaces = Some(old_hash.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> BroadcastEntry {
        BroadcastEntry {
            hash: "0xabc".into(),
            label: "Send".into(),
            chain_id: 943,
            from: "0x1111111111111111111111111111111111111111".into(),
            to: "0x2222222222222222222222222222222222222222".into(),
            value: "1000".into(),
            data: None,
            nonce: 7,
            gas_limit: Some(21_000),
            max_fee_per_gas: Some("1000".into()),
            max_priority_fee_per_gas: Some("100".into()),
            status: TxStatus::Pending,
            replaces: None,
        }
    }

    #[test]
    fn speed_up_keeps_payload_bumps_fees() {
        let e = sample_entry();
        let tx = e.replacement_tx(ReplaceKind::SpeedUp).unwrap();
        assert_eq!(tx.nonce, Some(7));
        assert_eq!(tx.to, e.to);
        assert_eq!(tx.value, e.value);
        let max = U256::from_str(tx.max_fee_per_gas.as_deref().unwrap()).unwrap();
        assert!(max > U256::from(1000u64));
    }

    #[test]
    fn cancel_is_zero_self_send() {
        let e = sample_entry();
        let tx = e.replacement_tx(ReplaceKind::Cancel).unwrap();
        assert_eq!(tx.to, e.from);
        assert_eq!(tx.value, "0");
        assert!(tx.data.is_none());
        assert_eq!(tx.nonce, Some(7));
    }

    #[test]
    fn push_recent_dedupes_and_caps() {
        let mut list = Vec::new();
        for i in 0..40 {
            push_recent(
                &mut list,
                BroadcastEntry {
                    hash: format!("0x{i}"),
                    ..sample_entry()
                },
            );
        }
        assert_eq!(list.len(), MAX_RECENT_BROADCASTS);
        assert_eq!(list[0].hash, "0x39");
    }
}
