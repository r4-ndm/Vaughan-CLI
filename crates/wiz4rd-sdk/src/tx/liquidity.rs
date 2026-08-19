//! Liquidity position transaction builders (NonfungiblePositionManager).
//!
//! Pure construction — no signing/sending. Amounts are in the tokens' smallest
//! units; ticks are the raw int24 values (see `wiz4rd-math::nearest_usable_tick`
//! for range parsing helpers).

use alloy::primitives::{aliases::I24, aliases::U24, Address, U256};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;

use crate::abi::INonfungiblePositionManager;
use crate::config::Config;
use crate::error::{SdkError, SdkResult};

fn fee_u24(fee: u32) -> SdkResult<U24> {
    U24::try_from(fee).map_err(|e| SdkError::Math(e.to_string()))
}

fn tick_i24(tick: i32) -> SdkResult<I24> {
    I24::try_from(tick).map_err(|e| SdkError::Math(e.to_string()))
}

/// Build a `mint` transaction: open a new position in the `[tick_lower,
/// tick_upper]` range, spending up to `amount0_desired`/`amount1_desired`.
///
/// The PositionManager computes the actual liquidity from the desired amounts
/// and current price; `amount0_min`/`amount1_min` are the slippage floors.
pub fn build_mint_tx(
    config: &Config,
    token0: Address,
    token1: Address,
    fee: u32,
    tick_lower: i32,
    tick_upper: i32,
    amount0_desired: U256,
    amount1_desired: U256,
    amount0_min: U256,
    amount1_min: U256,
    recipient: Address,
    deadline: u64,
) -> SdkResult<TransactionRequest> {
    let npm = config
        .position_manager
        .ok_or(SdkError::MissingAddress("position_manager"))?;

    let params = INonfungiblePositionManager::MintParams {
        token0,
        token1,
        fee: fee_u24(fee)?,
        tickLower: tick_i24(tick_lower)?,
        tickUpper: tick_i24(tick_upper)?,
        amount0Desired: amount0_desired,
        amount1Desired: amount1_desired,
        amount0Min: amount0_min,
        amount1Min: amount1_min,
        recipient,
        deadline: U256::from(deadline),
    };
    let call = INonfungiblePositionManager::mintCall { params };
    Ok(TransactionRequest::default().to(npm).input(call.abi_encode().into()))
}

/// Build an `increaseLiquidity` transaction for an existing `token_id`.
pub fn build_increase_liquidity_tx(
    config: &Config,
    token_id: U256,
    amount0_desired: U256,
    amount1_desired: U256,
    amount0_min: U256,
    amount1_min: U256,
    deadline: u64,
) -> SdkResult<TransactionRequest> {
    let npm = config
        .position_manager
        .ok_or(SdkError::MissingAddress("position_manager"))?;

    let params = INonfungiblePositionManager::IncreaseLiquidityParams {
        tokenId: token_id,
        amount0Desired: amount0_desired,
        amount1Desired: amount1_desired,
        amount0Min: amount0_min,
        amount1Min: amount1_min,
        deadline: U256::from(deadline),
    };
    let call = INonfungiblePositionManager::increaseLiquidityCall { params };
    Ok(TransactionRequest::default().to(npm).input(call.abi_encode().into()))
}

/// Build a `decreaseLiquidity` transaction: remove `liquidity` from `token_id`
/// (burning it back into owed tokens), with slippage floors on the amounts.
pub fn build_decrease_liquidity_tx(
    config: &Config,
    token_id: U256,
    liquidity: u128,
    amount0_min: U256,
    amount1_min: U256,
    deadline: u64,
) -> SdkResult<TransactionRequest> {
    let npm = config
        .position_manager
        .ok_or(SdkError::MissingAddress("position_manager"))?;

    let params = INonfungiblePositionManager::DecreaseLiquidityParams {
        tokenId: token_id,
        liquidity,
        amount0Min: amount0_min,
        amount1Min: amount1_min,
        deadline: U256::from(deadline),
    };
    let call = INonfungiblePositionManager::decreaseLiquidityCall { params };
    Ok(TransactionRequest::default().to(npm).input(call.abi_encode().into()))
}

/// Build a `collect` transaction: withdraw owed tokens (fees + decreased
/// liquidity) from `token_id` to `recipient`.
pub fn build_collect_tx(
    config: &Config,
    token_id: U256,
    recipient: Address,
    amount0_max: u128,
    amount1_max: u128,
) -> SdkResult<TransactionRequest> {
    let npm = config
        .position_manager
        .ok_or(SdkError::MissingAddress("position_manager"))?;

    let params = INonfungiblePositionManager::CollectParams {
        tokenId: token_id,
        recipient,
        amount0Max: amount0_max,
        amount1Max: amount1_max,
    };
    let call = INonfungiblePositionManager::collectCall { params };
    Ok(TransactionRequest::default().to(npm).input(call.abi_encode().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            position_manager: Some(Address::repeat_byte(0xbb)),
            ..Config::default()
        }
    }

    #[test]
    fn mint_encodes_selector_and_address() {
        let tx = build_mint_tx(
            &cfg(),
            Address::repeat_byte(0x11),
            Address::repeat_byte(0x22),
            500,
            -600,
            600,
            U256::from(10u128.pow(18)),
            U256::from(10u128.pow(18)),
            U256::ZERO,
            U256::ZERO,
            Address::repeat_byte(0xee),
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(tx.to, Some(alloy::primitives::TxKind::Call(Address::repeat_byte(0xbb))));
        let data = tx.input.into_input().unwrap();
        assert_eq!(&data[..4], &INonfungiblePositionManager::mintCall::SELECTOR);
    }

    #[test]
    fn decrease_and_collect_have_distinct_selectors() {
        let dec = build_decrease_liquidity_tx(
            &cfg(),
            U256::from(1u64),
            1000,
            U256::ZERO,
            U256::ZERO,
            1_700_000_000,
        )
        .unwrap();
        let col = build_collect_tx(&cfg(), U256::from(1u64), Address::repeat_byte(0xee), u128::MAX, u128::MAX)
            .unwrap();
        let d = dec.input.into_input().unwrap();
        let c = col.input.into_input().unwrap();
        assert_eq!(&d[..4], &INonfungiblePositionManager::decreaseLiquidityCall::SELECTOR);
        assert_eq!(&c[..4], &INonfungiblePositionManager::collectCall::SELECTOR);
    }

    #[test]
    fn missing_position_manager_errors() {
        let cfg = Config::default(); // no position_manager
        assert!(build_collect_tx(&cfg, U256::from(1u64), Address::repeat_byte(0xee), u128::MAX, u128::MAX).is_err());
    }
}
