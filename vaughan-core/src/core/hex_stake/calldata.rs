//! Encode HEX `stakeStart` / `stakeEnd` calldata.

use alloy::primitives::{Bytes, U256};
use alloy::sol;
use alloy::sol_types::SolCall;

use crate::error::WalletError;

sol! {
    /// Minimal HEX stake write surface (Ethereum HEX / PulseChain state-fork).
    interface IHexStakeWrite {
        function stakeStart(uint256 newStakedHearts, uint256 newStakedDays) external returns (uint40);
        function stakeEnd(uint256 stakeIndex, uint40 stakeId) external;
    }
}

/// HEX hearts decimals.
pub const PHEX_HEARTS_DECIMALS: u8 = 8;

/// Minimum stake length in HEX days.
pub const MIN_STAKE_DAYS: u64 = 1;

/// Maximum stake length in HEX days (protocol cap).
pub const MAX_STAKE_DAYS: u64 = 5555;

/// Encode `stakeStart(hearts, days)`.
pub fn encode_stake_start(hearts: U256, staked_days: u64) -> Result<Bytes, WalletError> {
    if hearts.is_zero() {
        return Err(WalletError::InvalidAmount(
            "HEX stake hearts must be > 0".into(),
        ));
    }
    if !(MIN_STAKE_DAYS..=MAX_STAKE_DAYS).contains(&staked_days) {
        return Err(WalletError::InvalidTransaction(format!(
            "HEX stake days must be {MIN_STAKE_DAYS}–{MAX_STAKE_DAYS}, got {staked_days}"
        )));
    }
    Ok(Bytes::from(
        IHexStakeWrite::stakeStartCall {
            newStakedHearts: hearts,
            newStakedDays: U256::from(staked_days),
        }
        .abi_encode(),
    ))
}

/// Encode `stakeEnd(index, stakeId)`.
pub fn encode_stake_end(stake_index: u64, stake_id: u64) -> Result<Bytes, WalletError> {
    if stake_id >= (1u64 << 40) {
        return Err(WalletError::InvalidTransaction(
            "HEX stakeId exceeds uint40".into(),
        ));
    }
    Ok(Bytes::from(
        IHexStakeWrite::stakeEndCall {
            stakeIndex: U256::from(stake_index),
            stakeId: alloy::primitives::aliases::U40::from(stake_id),
        }
        .abi_encode(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    #[test]
    fn stake_start_rejects_bad_days() {
        assert!(encode_stake_start(U256::from(1u64), 0).is_err());
        assert!(encode_stake_start(U256::from(1u64), 5556).is_err());
        assert!(encode_stake_start(U256::ZERO, 100).is_err());
        assert_eq!(encode_stake_start(U256::from(1u64), 365).unwrap().len(), 68);
    }

    #[test]
    fn stake_end_encodes_selector() {
        let data = encode_stake_end(0, 42).unwrap();
        assert_eq!(data.len(), 68);
    }
}
