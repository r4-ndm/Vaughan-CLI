//! Uniswap V2–style LP (pair tokens) — 9inch on Pulse mainnet today.
//!
//! Browserless add / remove / list using catalogued factory + V2 router.
//! Pair LP tokens are plain ERC-20 balances on the pair contract address.

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;
use alloy::sol_types::SolCall;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::chains::EvmTransaction;
use crate::core::dex_catalog::{venue_swap_router, venue_v2_factory, DexProtocol, DexVenue};
use crate::core::dex_quote::min_out_after_slippage;
use crate::core::dex_routers::is_allowed_dex_router;
use crate::core::transaction::parse_native_amount;
use crate::error::WalletError;

/// A V2 LP stake: LP token balance on the pair contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2LpPosition {
    pub pair: Address,
    pub token0: Address,
    pub token1: Address,
    pub lp_balance: U256,
}

sol! {
    interface IUniswapV2Factory {
        function getPair(address tokenA, address tokenB) external view returns (address pair);
    }

    interface IUniswapV2Pair {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function balanceOf(address account) external view returns (uint256);
    }

    interface IUniswapV2RouterLiquidity {
        function addLiquidity(
            address tokenA,
            address tokenB,
            uint256 amountADesired,
            uint256 amountBDesired,
            uint256 amountAMin,
            uint256 amountBMin,
            address to,
            uint256 deadline
        ) external returns (uint256 amountA, uint256 amountB, uint256 liquidity);

        function addLiquidityETH(
            address token,
            uint256 amountTokenDesired,
            uint256 amountTokenMin,
            uint256 amountETHMin,
            address to,
            uint256 deadline
        ) external payable returns (uint256 amountToken, uint256 amountETH, uint256 liquidity);

        function removeLiquidity(
            address tokenA,
            address tokenB,
            uint256 liquidity,
            uint256 amountAMin,
            uint256 amountBMin,
            address to,
            uint256 deadline
        ) external returns (uint256 amountA, uint256 amountB);

        function removeLiquidityETH(
            address token,
            uint256 liquidity,
            uint256 amountTokenMin,
            uint256 amountETHMin,
            address to,
            uint256 deadline
        ) external returns (uint256 amountToken, uint256 amountETH);
    }
}

fn connect_http(rpc_url: &str) -> Result<impl Provider + use<>, WalletError> {
    let url = rpc_url
        .trim()
        .parse()
        .map_err(|e| WalletError::NetworkError(format!("invalid RPC URL: {e}")))?;
    Ok(ProviderBuilder::new().connect_http(url))
}

fn default_deadline() -> U256 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_add(1200))
        .unwrap_or(0);
    U256::from(secs)
}

fn sort_pair(a: Address, b: Address) -> (Address, Address) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

fn assert_v2_router(venue: DexVenue, chain_id: u64) -> Result<Address, WalletError> {
    let router = venue_swap_router(venue, DexProtocol::V2, chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V2 router on chain {chain_id}",
            venue.label()
        ))
    })?;
    if !is_allowed_dex_router(chain_id, router) {
        return Err(WalletError::InvalidTransaction(format!(
            "router {router:#x} is not allowlisted on chain {chain_id}"
        )));
    }
    Ok(router)
}

fn v2_factory(venue: DexVenue, chain_id: u64) -> Result<Address, WalletError> {
    venue_v2_factory(venue, chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V2 factory on chain {chain_id}",
            venue.label()
        ))
    })
}

/// Default token pairs to probe when listing 9inch V2 LP on mainnet.
pub fn default_v2_watch_pairs(chain_id: u64, venue: DexVenue) -> Vec<(Address, Address)> {
    if chain_id != 369 || venue != DexVenue::NineInch {
        return Vec::new();
    }
    let wpls = match super::dex_routers::wpls_for_chain(chain_id) {
        Some(w) => w,
        None => return Vec::new(),
    };
    let hex = Address::from_str("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39").ok();
    hex.map(|h| vec![sort_pair(wpls, h)]).unwrap_or_default()
}

/// Resolve the pair contract for two tokens (sorted internally).
pub async fn get_v2_pair_address(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    token_a: Address,
    token_b: Address,
) -> Result<Address, WalletError> {
    let factory = v2_factory(venue, chain_id)?;
    let (a, b) = sort_pair(token_a, token_b);
    let provider = connect_http(rpc_url)?;
    let call = IUniswapV2Factory::getPairCall {
        tokenA: a,
        tokenB: b,
    };
    let raw = provider
        .call(
            alloy::rpc::types::TransactionRequest::default()
                .to(factory)
                .input(call.abi_encode().into()),
        )
        .await
        .map_err(|e| WalletError::NetworkError(format!("getPair: {e}")))?;
    let pair = IUniswapV2Factory::getPairCall::abi_decode_returns(&raw)
        .map_err(|e| WalletError::NetworkError(format!("decode getPair: {e}")))?;
    if pair.is_zero() {
        return Err(WalletError::NetworkError(
            "pair does not exist for this token pair".into(),
        ));
    }
    Ok(pair)
}

async fn read_v2_lp_balance(
    provider: &impl Provider,
    pair: Address,
    owner: Address,
) -> Result<U256, WalletError> {
    let call = IUniswapV2Pair::balanceOfCall { account: owner };
    let raw = provider
        .call(
            alloy::rpc::types::TransactionRequest::default()
                .to(pair)
                .input(call.abi_encode().into()),
        )
        .await
        .map_err(|e| WalletError::NetworkError(format!("pair balanceOf: {e}")))?;
    IUniswapV2Pair::balanceOfCall::abi_decode_returns(&raw)
        .map_err(|e| WalletError::NetworkError(format!("decode balanceOf: {e}")))
}

/// List V2 LP positions for `owner` across `watch_pairs` (skips zero balances).
pub async fn list_v2_lp_positions(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    owner: Address,
    watch_pairs: &[(Address, Address)],
) -> Result<Vec<V2LpPosition>, WalletError> {
    let provider = connect_http(rpc_url)?;
    let mut out = Vec::new();
    for &(ta, tb) in watch_pairs {
        let pair = match get_v2_pair_address(rpc_url, venue, chain_id, ta, tb).await {
            Ok(p) => p,
            Err(_) => continue,
        };
        let bal = read_v2_lp_balance(&provider, pair, owner).await?;
        if bal.is_zero() {
            continue;
        }
        let t0_call = IUniswapV2Pair::token0Call {};
        let t1_call = IUniswapV2Pair::token1Call {};
        let t0_raw = provider
            .call(
                alloy::rpc::types::TransactionRequest::default()
                    .to(pair)
                    .input(t0_call.abi_encode().into()),
            )
            .await
            .map_err(|e| WalletError::NetworkError(format!("token0: {e}")))?;
        let t1_raw = provider
            .call(
                alloy::rpc::types::TransactionRequest::default()
                    .to(pair)
                    .input(t1_call.abi_encode().into()),
            )
            .await
            .map_err(|e| WalletError::NetworkError(format!("token1: {e}")))?;
        let token0 = IUniswapV2Pair::token0Call::abi_decode_returns(&t0_raw)
            .map_err(|e| WalletError::NetworkError(format!("decode token0: {e}")))?;
        let token1 = IUniswapV2Pair::token1Call::abi_decode_returns(&t1_raw)
            .map_err(|e| WalletError::NetworkError(format!("decode token1: {e}")))?;
        out.push(V2LpPosition {
            pair,
            token0,
            token1,
            lp_balance: bal,
        });
    }
    Ok(out)
}

fn parse_human_amount(raw: &str, decimals: u8, label: &str) -> Result<U256, WalletError> {
    let s = parse_native_amount(raw.trim(), decimals)?;
    U256::from_str(&s).map_err(|_| WalletError::InvalidAmount(format!("invalid {label}")))
}

/// Build V2 add-liquidity tx (ERC-20 pair or native+token via WPLS path).
#[allow(clippy::too_many_arguments)]
pub fn build_v2_add_liquidity_evm(
    from: &str,
    venue: DexVenue,
    chain_id: u64,
    token_a: Address,
    token_b: Address,
    amount_a_human: &str,
    amount_b_human: &str,
    decimals_a: u8,
    decimals_b: u8,
    slippage_bps: u32,
    native_side: Option<Address>,
) -> Result<EvmTransaction, WalletError> {
    let router = assert_v2_router(venue, chain_id)?;
    let recipient = Address::from_str(from)
        .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?;
    let amount_a = parse_human_amount(amount_a_human, decimals_a, "amount_a")?;
    let amount_b = parse_human_amount(amount_b_human, decimals_b, "amount_b")?;
    let deadline = default_deadline();
    let (data, value) = if let Some(wpls) = native_side {
        let (token, amount_token, amount_eth) = if token_a == wpls {
            (token_b, amount_b, amount_a)
        } else if token_b == wpls {
            (token_a, amount_a, amount_b)
        } else {
            return Err(WalletError::InvalidTransaction(
                "native add requires WPLS as one side".into(),
            ));
        };
        let call = IUniswapV2RouterLiquidity::addLiquidityETHCall {
            token,
            amountTokenDesired: amount_token,
            amountTokenMin: min_out_after_slippage(amount_token, slippage_bps),
            amountETHMin: min_out_after_slippage(amount_eth, slippage_bps),
            to: recipient,
            deadline,
        };
        (call.abi_encode(), amount_eth)
    } else {
        let (ta, tb) = sort_pair(token_a, token_b);
        let (amount_ad, amount_bd) = if token_a == ta && token_b == tb {
            (amount_a, amount_b)
        } else {
            (amount_b, amount_a)
        };
        let call = IUniswapV2RouterLiquidity::addLiquidityCall {
            tokenA: ta,
            tokenB: tb,
            amountADesired: amount_ad,
            amountBDesired: amount_bd,
            amountAMin: min_out_after_slippage(amount_ad, slippage_bps),
            amountBMin: min_out_after_slippage(amount_bd, slippage_bps),
            to: recipient,
            deadline,
        };
        (call.abi_encode(), U256::ZERO)
    };
    Ok(EvmTransaction {
        from: from.to_string(),
        to: format!("{router:#x}"),
        value: value.to_string(),
        data: Some(format!("0x{}", hex::encode(data))),
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id,
    })
}

/// Build V2 remove-liquidity tx (burn LP tokens held on pair contract).
#[allow(clippy::too_many_arguments)]
pub fn build_v2_remove_liquidity_evm(
    from: &str,
    venue: DexVenue,
    chain_id: u64,
    token0: Address,
    token1: Address,
    liquidity: U256,
    _slippage_bps: u32,
    native_side: Option<Address>,
) -> Result<EvmTransaction, WalletError> {
    let router = assert_v2_router(venue, chain_id)?;
    let recipient = Address::from_str(from)
        .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?;
    let deadline = default_deadline();
    let (ta, tb) = sort_pair(token0, token1);
    let data = if let Some(wpls) = native_side {
        let token = if token0 == wpls {
            token1
        } else if token1 == wpls {
            token0
        } else {
            return Err(WalletError::InvalidTransaction(
                "native remove requires WPLS side".into(),
            ));
        };
        IUniswapV2RouterLiquidity::removeLiquidityETHCall {
            token,
            liquidity,
            amountTokenMin: U256::ZERO,
            amountETHMin: U256::ZERO,
            to: recipient,
            deadline,
        }
        .abi_encode()
    } else {
        IUniswapV2RouterLiquidity::removeLiquidityCall {
            tokenA: ta,
            tokenB: tb,
            liquidity,
            amountAMin: U256::ZERO,
            amountBMin: U256::ZERO,
            to: recipient,
            deadline,
        }
        .abi_encode()
    };
    Ok(EvmTransaction {
        from: from.to_string(),
        to: format!("{router:#x}"),
        value: "0".into(),
        data: Some(format!("0x{}", hex::encode(data))),
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_inch_factory_catalogued_on_369() {
        assert_eq!(
            venue_v2_factory(DexVenue::NineInch, 369),
            Some(Address::from_str("0x5b9F077A77db37F3Be0A5b5d31BAeff4bc5C0bD7").unwrap())
        );
    }

    #[test]
    fn add_liquidity_calldata_targets_router() {
        let wpls = Address::from_str("0xA1077a294dDE1B09bB078844df40758a5D0f9a27").unwrap();
        let hex = Address::from_str("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39").unwrap();
        let tx = build_v2_add_liquidity_evm(
            "0x0000000000000000000000000000000000000001",
            DexVenue::NineInch,
            369,
            wpls,
            hex,
            "1",
            "100",
            18,
            8,
            50,
            None,
        )
        .unwrap();
        assert_eq!(
            tx.to.to_lowercase(),
            "0xeb45a3c4aedd0f47f345fb4c8a1802bb5740d725"
        );
        assert!(tx.data.as_ref().unwrap().starts_with("0x"));
    }
}
