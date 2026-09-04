//! On-chain HEX stake reads via `eth_call` (soft-fail).

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use std::str::FromStr;

use crate::error::WalletError;

use super::contract::{resolve_hex_contract, HexContractRef};
use super::types::{
    HexGlobalState, HexSoftFail, HexStakeResult, HexStakeRow, HexStakesForAddress,
};

sol! {
    /// Minimal HEX stake view surface (Ethereum HEX / PulseChain state-fork).
    interface IHexStakeView {
        function currentDay() external view returns (uint256);
        function stakeCount(address stakerAddr) external view returns (uint256);
        function globals()
            external
            view
            returns (
                uint72 lockedHeartsTotal,
                uint72 nextStakeSharesTotal,
                uint40 shareRate,
                uint72 stakePenaltyTotal,
                uint16 dailyDataCount,
                uint72 stakeSharesTotal,
                uint40 latestStakeId,
                uint128 claimStats
            );
        function stakeLists(address stakerAddr, uint256 stakeIndex)
            external
            view
            returns (
                uint40 stakeId,
                uint72 stakedHearts,
                uint72 stakeShares,
                uint16 lockedDay,
                uint16 stakedDays,
                uint16 unlockedDay,
                bool isAutoStake
            );
    }
}

fn connect_http(rpc_url: &str) -> Result<impl Provider + use<>, WalletError> {
    let url = rpc_url
        .trim()
        .parse()
        .map_err(|e| WalletError::RpcError(format!("bad rpc url: {e}")))?;
    Ok(ProviderBuilder::new().connect_http(url))
}

async fn eth_call_raw(
    provider: &impl Provider,
    to: Address,
    data: alloy::primitives::Bytes,
) -> Result<alloy::primitives::Bytes, WalletError> {
    let req = TransactionRequest::default().to(to).input(data.into());
    provider
        .call(req)
        .await
        .map_err(|e| WalletError::RpcError(format!("hex eth_call failed: {e}")))
}

/// Read HEX global stake state (`currentDay` + `globals`).
pub async fn fetch_hex_global_state(
    rpc_url: &str,
    which: &str,
) -> HexStakeResult<HexGlobalState> {
    let contract = match resolve_hex_contract(which) {
        Ok(c) => c,
        Err(e) => {
            return HexStakeResult::fail(HexSoftFail::new(e, Some("resolveHexContract")));
        }
    };
    if !contract.supports_staking {
        return HexStakeResult::fail(
            HexSoftFail::new(contract.note, Some("globals")).with_contract(&contract),
        );
    }

    match fetch_globals_inner(rpc_url, &contract).await {
        Ok(data) => HexStakeResult::success(&contract, data),
        Err(e) => HexStakeResult::fail(
            HexSoftFail::new(e.user_message(), Some("currentDay+globals")).with_contract(&contract),
        ),
    }
}

async fn fetch_globals_inner(
    rpc_url: &str,
    contract: &HexContractRef,
) -> Result<HexGlobalState, WalletError> {
    let provider = connect_http(rpc_url)?;
    let day_data = eth_call_raw(
        &provider,
        contract.address,
        IHexStakeView::currentDayCall {}.abi_encode().into(),
    )
    .await?;
    let current_day = IHexStakeView::currentDayCall::abi_decode_returns(&day_data)
        .map_err(|e| WalletError::RpcError(format!("currentDay decode: {e}")))?;

    let g_data = eth_call_raw(
        &provider,
        contract.address,
        IHexStakeView::globalsCall {}.abi_encode().into(),
    )
    .await?;
    let g = IHexStakeView::globalsCall::abi_decode_returns(&g_data)
        .map_err(|e| WalletError::RpcError(format!("globals decode: {e}")))?;

    Ok(HexGlobalState {
        current_day: current_day.to_string(),
        locked_hearts_total: g.lockedHeartsTotal.to_string(),
        next_stake_shares_total: g.nextStakeSharesTotal.to_string(),
        share_rate: g.shareRate.to_string(),
        stake_penalty_total: g.stakePenaltyTotal.to_string(),
        daily_data_count: g.dailyDataCount.to_string(),
        stake_shares_total: g.stakeSharesTotal.to_string(),
        latest_stake_id: g.latestStakeId.to_string(),
        claim_stats: g.claimStats.to_string(),
        hearts_decimals: 8,
        note: "On-chain HEX global state. Hearts use 8 decimals. Not a price oracle; \
               shareRate is protocol-internal.",
    })
}

/// List stakes for `staker` via `stakeCount` + `stakeLists`.
pub async fn fetch_hex_stakes_for_address(
    rpc_url: &str,
    staker: &str,
    which: &str,
    limit: usize,
) -> HexStakeResult<HexStakesForAddress> {
    let staker_addr = match Address::from_str(staker.trim()) {
        Ok(a) => a,
        Err(_) => {
            return HexStakeResult::fail(HexSoftFail::new(
                "staker must be a 0x-prefixed 40-hex address",
                Some("staker"),
            ));
        }
    };
    let contract = match resolve_hex_contract(which) {
        Ok(c) => c,
        Err(e) => {
            return HexStakeResult::fail(HexSoftFail::new(e, Some("resolveHexContract")));
        }
    };
    if !contract.supports_staking {
        return HexStakeResult::fail(
            HexSoftFail::new(contract.note, Some("stakeLists")).with_contract(&contract),
        );
    }

    let limit = limit.clamp(1, 100);
    match fetch_stakes_inner(rpc_url, &contract, staker_addr, limit).await {
        Ok(data) => HexStakeResult::success(&contract, data),
        Err(e) => HexStakeResult::fail(
            HexSoftFail::new(e.user_message(), Some("stakeCount+stakeLists"))
                .with_contract(&contract),
        ),
    }
}

async fn fetch_stakes_inner(
    rpc_url: &str,
    contract: &HexContractRef,
    staker: Address,
    limit: usize,
) -> Result<HexStakesForAddress, WalletError> {
    let provider = connect_http(rpc_url)?;
    let count_data = eth_call_raw(
        &provider,
        contract.address,
        IHexStakeView::stakeCountCall {
            stakerAddr: staker,
        }
        .abi_encode()
        .into(),
    )
    .await?;
    let count = IHexStakeView::stakeCountCall::abi_decode_returns(&count_data)
        .map_err(|e| WalletError::RpcError(format!("stakeCount decode: {e}")))?;
    let count_n = count.min(U256::from(u32::MAX)).to::<u32>() as usize;
    let n = count_n.min(limit);
    let mut stakes = Vec::with_capacity(n);
    for i in 0..n {
        let row_data = eth_call_raw(
            &provider,
            contract.address,
            IHexStakeView::stakeListsCall {
                stakerAddr: staker,
                stakeIndex: U256::from(i),
            }
            .abi_encode()
            .into(),
        )
        .await?;
        let row = IHexStakeView::stakeListsCall::abi_decode_returns(&row_data)
            .map_err(|e| WalletError::RpcError(format!("stakeLists[{i}] decode: {e}")))?;
        let unlocked_day = u32::from(row.unlockedDay);
        stakes.push(HexStakeRow {
            index: i as u32,
            stake_id: row.stakeId.to_string(),
            staked_hearts: row.stakedHearts.to_string(),
            stake_shares: row.stakeShares.to_string(),
            locked_day: u32::from(row.lockedDay),
            staked_days: u32::from(row.stakedDays),
            unlocked_day,
            is_auto_stake: row.isAutoStake,
            still_locked: unlocked_day == 0,
        });
    }

    Ok(HexStakesForAddress {
        staker: format!("{staker:#x}"),
        stake_count: count.to_string(),
        stakes,
        truncated: count_n > limit,
        hearts_decimals: 8,
        note: "On-chain HEX stakeLists. stakedHearts are Hearts (8 decimals). \
               stillLocked=true when unlockedDay is 0. Not a price oracle.",
        staker_addr: staker,
    })
}
