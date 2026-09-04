//! Soft-fail envelopes and stake row shapes (serde for MCP).

use alloy::primitives::Address;
use serde::Serialize;

use super::contract::HexContractRef;

/// Source label for soft-fail / success envelopes.
pub const HEX_STAKE_SOURCE: &str = "hex-rpc";

/// Soft-fail envelope (never invents stake state).
#[derive(Debug, Clone, Serialize)]
pub struct HexSoftFail {
    pub ok: bool,
    pub source: &'static str,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_label: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<&'static str>,
}

impl HexSoftFail {
    pub fn new(reason: impl Into<String>, path: Option<&'static str>) -> Self {
        Self {
            ok: false,
            source: HEX_STAKE_SOURCE,
            reason: reason.into(),
            contract_label: None,
            path,
        }
    }

    pub fn with_contract(mut self, c: &HexContractRef) -> Self {
        self.contract_label = Some(c.label);
        self
    }
}

/// Success or soft-fail for HEX stake reads.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum HexStakeResult<T: Serialize> {
    Ok {
        ok: bool,
        source: &'static str,
        contract_label: &'static str,
        contract: String,
        data: T,
    },
    Err(HexSoftFail),
}

impl<T: Serialize> HexStakeResult<T> {
    pub fn success(contract: &HexContractRef, data: T) -> Self {
        Self::Ok {
            ok: true,
            source: HEX_STAKE_SOURCE,
            contract_label: contract.label,
            contract: format!("{:#x}", contract.address),
            data,
        }
    }

    pub fn fail(f: HexSoftFail) -> Self {
        Self::Err(f)
    }
}

/// On-chain HEX `globals()` + `currentDay`.
#[derive(Debug, Clone, Serialize)]
pub struct HexGlobalState {
    pub current_day: String,
    pub locked_hearts_total: String,
    pub next_stake_shares_total: String,
    pub share_rate: String,
    pub stake_penalty_total: String,
    pub daily_data_count: String,
    pub stake_shares_total: String,
    pub latest_stake_id: String,
    pub claim_stats: String,
    pub hearts_decimals: u8,
    pub note: &'static str,
}

/// One `stakeLists` row.
#[derive(Debug, Clone, Serialize)]
pub struct HexStakeRow {
    pub index: u32,
    pub stake_id: String,
    pub staked_hearts: String,
    pub stake_shares: String,
    pub locked_day: u32,
    pub staked_days: u32,
    pub unlocked_day: u32,
    pub is_auto_stake: bool,
    /// `unlocked_day == 0` means still locked (HEX convention).
    pub still_locked: bool,
}

/// Stakes listed for one address.
#[derive(Debug, Clone, Serialize)]
pub struct HexStakesForAddress {
    pub staker: String,
    pub stake_count: String,
    pub stakes: Vec<HexStakeRow>,
    pub truncated: bool,
    pub hearts_decimals: u8,
    pub note: &'static str,
    /// Convenience for TUI (same as parsed staker).
    #[serde(skip)]
    pub staker_addr: Address,
}
