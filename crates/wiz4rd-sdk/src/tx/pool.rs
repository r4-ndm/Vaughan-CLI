//! V3 pool deployment tx builders (factory `createPool` + pool `initialize`).
//!
//! PancakeSwap V3 fork — same ABI as wiz4rd / 9inch / 9mm on PulseChain.

use alloy::primitives::{aliases::U24, Address, U160};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;

use crate::abi::{IPancakeV3Factory, IPancakeV3Pool};
use crate::config::Config;
use crate::error::{SdkError, SdkResult};
use crate::pool_address::get_pool_key;

fn fee_u24(fee: u32) -> SdkResult<U24> {
    U24::try_from(fee).map_err(|e| SdkError::Math(e.to_string()))
}

/// Build a factory `createPool(tokenA, tokenB, fee)` transaction.
pub fn build_create_pool_tx(
    config: &Config,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> SdkResult<TransactionRequest> {
    let factory = config.factory.ok_or(SdkError::MissingAddress("factory"))?;
    let key = get_pool_key(token_a, token_b, fee);
    let call = IPancakeV3Factory::createPoolCall {
        tokenA: key.token0,
        tokenB: key.token1,
        fee: fee_u24(fee)?,
    };
    Ok(TransactionRequest::default()
        .to(factory)
        .input(call.abi_encode().into()))
}

/// Build a pool `initialize(sqrtPriceX96)` transaction.
pub fn build_initialize_pool_tx(
    pool: Address,
    sqrt_price_x96: U160,
) -> SdkResult<TransactionRequest> {
    let call = IPancakeV3Pool::initializeCall {
        sqrtPriceX96: sqrt_price_x96,
    };
    Ok(TransactionRequest::default()
        .to(pool)
        .input(call.abi_encode().into()))
}
