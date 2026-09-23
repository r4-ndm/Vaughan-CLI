//! Uniswap V2–style LP (pair tokens) — 9inch on Pulse, LFG/UniWswap/PowSwap/Uniswap on ETHW.
//!
//! Browserless add / remove / list using catalogued factory + V2 router.
//! Pair LP tokens are plain ERC-20 balances on the pair contract address.
//! On ETHW, LFG/UniWswap/PowSwap listing also walks `allPairs` so non-HEX pools show up.

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;
use alloy::sol_types::SolCall;
use futures_util::stream::{self, StreamExt};
use std::collections::HashSet;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::chains::EvmTransaction;
use crate::core::dex_catalog::{venue_swap_router, venue_v2_factory, DexProtocol, DexVenue};
use crate::core::dex_quote::min_out_after_slippage;
use crate::core::dex_routers::is_allowed_dex_router;
use crate::core::transaction::parse_native_amount;
use crate::error::WalletError;

/// A V2 LP stake: LP balance plus pool reserves / supply for share + underlying amounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2LpPosition {
    pub pair: Address,
    pub token0: Address,
    pub token1: Address,
    pub lp_balance: U256,
    pub reserve0: U256,
    pub reserve1: U256,
    pub total_supply: U256,
}

impl V2LpPosition {
    /// Underlying token amounts owed for this LP stake (`lp * reserve / supply`).
    pub fn underlying_amounts(&self) -> (U256, U256) {
        v2_underlying_amounts(
            self.lp_balance,
            self.total_supply,
            self.reserve0,
            self.reserve1,
        )
    }

    /// Pool share in basis points (10_000 = 100%). Zero if supply is zero.
    pub fn pool_share_bps(&self) -> u32 {
        v2_pool_share_bps(self.lp_balance, self.total_supply)
    }
}

/// `amount_i = lp * reserve_i / total_supply` (zero if supply is zero).
pub fn v2_underlying_amounts(
    lp_balance: U256,
    total_supply: U256,
    reserve0: U256,
    reserve1: U256,
) -> (U256, U256) {
    if total_supply.is_zero() || lp_balance.is_zero() {
        return (U256::ZERO, U256::ZERO);
    }
    (
        lp_balance.saturating_mul(reserve0) / total_supply,
        lp_balance.saturating_mul(reserve1) / total_supply,
    )
}

/// Pool ownership in basis points (`10_000` = 100%). Caps at `10_000`.
pub fn v2_pool_share_bps(lp_balance: U256, total_supply: U256) -> u32 {
    if total_supply.is_zero() || lp_balance.is_zero() {
        return 0;
    }
    let bps = (lp_balance.saturating_mul(U256::from(10_000u64))) / total_supply;
    u32::try_from(bps).unwrap_or(u32::MAX).min(10_000)
}

/// Human pool spot: how many token1 per 1 token0 from reserves (not an oracle).
pub fn v2_spot_token1_per_token0(
    reserve0: U256,
    reserve1: U256,
    decimals0: u8,
    decimals1: u8,
) -> Option<String> {
    use crate::core::transaction::format_display_amount;
    if reserve0.is_zero() {
        return None;
    }
    // price * 10^dec1 = reserve1 * 10^dec0 / reserve0
    let scale0 = U256::from(10u64).pow(U256::from(decimals0));
    let raw = reserve1.saturating_mul(scale0) / reserve0;
    Some(format_display_amount(&raw.to_string(), decimals1, 8))
}

sol! {
    interface IUniswapV2Factory {
        function getPair(address tokenA, address tokenB) external view returns (address pair);
        function allPairs(uint256 index) external view returns (address pair);
        function allPairsLength() external view returns (uint256);
    }

    interface IUniswapV2Pair {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function balanceOf(address account) external view returns (uint256);
        function totalSupply() external view returns (uint256);
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
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

fn connect_http(rpc_url: &str) -> Result<impl Provider + Clone + use<>, WalletError> {
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

/// Default token pairs to probe when listing V2 LP (HEX pools on Pulse + ETHW).
pub fn default_v2_watch_pairs(chain_id: u64, venue: DexVenue) -> Vec<(Address, Address)> {
    let hex = match Address::from_str("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39") {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };
    match (chain_id, venue) {
        (369, DexVenue::NineInch) => super::dex_routers::wpls_for_chain(chain_id)
            .map(|wpls| vec![sort_pair(wpls, hex)])
            .unwrap_or_default(),
        (10_001, DexVenue::LfgSwap) => {
            let mut pairs = Vec::new();
            if let Some(wethw) = super::dex_routers::wpls_for_chain(chain_id) {
                pairs.push(sort_pair(wethw, hex));
            }
            if let Ok(weth) = Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2") {
                pairs.push(sort_pair(weth, hex));
            }
            pairs
        }
        (10_001, DexVenue::PowSwap | DexVenue::UniHedron | DexVenue::UniWswap) => {
            Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2")
                .map(|weth| vec![sort_pair(weth, hex)])
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
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

/// Walk the factory index when it is small enough (LFG ~1.5k, UniW ~1.3k, PowSwap ~600).
const V2_FACTORY_SCAN_MAX: u64 = 2_500;
const V2_FACTORY_SCAN_CONCURRENCY: usize = 24;

/// List V2 LP positions for `owner` across `watch_pairs` (skips zero balances).
///
/// On factories with `allPairsLength <= 2500`, also scans every pair so LP
/// outside the default HEX watch list still appears.
pub async fn list_v2_lp_positions(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    owner: Address,
    watch_pairs: &[(Address, Address)],
) -> Result<Vec<V2LpPosition>, WalletError> {
    let provider = connect_http(rpc_url)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for &(ta, tb) in watch_pairs {
        let pair = match get_v2_pair_address(rpc_url, venue, chain_id, ta, tb).await {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !seen.insert(pair) {
            continue;
        }
        if let Some(pos) = read_v2_position_if_held(&provider, pair, owner).await? {
            out.push(pos);
        }
    }

    let factory_pairs = factory_pairs_to_scan(&provider, venue, chain_id).await?;
    let to_scan: Vec<Address> = factory_pairs
        .into_iter()
        .filter(|pair| seen.insert(*pair))
        .collect();
    let extras: Vec<Result<Option<V2LpPosition>, WalletError>> = stream::iter(to_scan)
        .map(|pair| {
            let provider = provider.clone();
            async move { read_v2_position_if_held(&provider, pair, owner).await }
        })
        .buffer_unordered(V2_FACTORY_SCAN_CONCURRENCY)
        .collect()
        .await;
    for item in extras {
        if let Some(pos) = item? {
            out.push(pos);
        }
    }
    Ok(out)
}

async fn factory_pairs_to_scan<P: Provider + Clone>(
    provider: &P,
    venue: DexVenue,
    chain_id: u64,
) -> Result<Vec<Address>, WalletError> {
    let factory = match venue_v2_factory(venue, chain_id) {
        Some(f) => f,
        None => return Ok(Vec::new()),
    };
    let len_raw = match provider
        .call(
            alloy::rpc::types::TransactionRequest::default()
                .to(factory)
                .input(IUniswapV2Factory::allPairsLengthCall {}.abi_encode().into()),
        )
        .await
    {
        Ok(raw) => raw,
        Err(_) => return Ok(Vec::new()),
    };
    let n = match IUniswapV2Factory::allPairsLengthCall::abi_decode_returns(&len_raw) {
        Ok(v) => u64::try_from(v).unwrap_or(u64::MAX),
        Err(_) => return Ok(Vec::new()),
    };
    if n == 0 || n > V2_FACTORY_SCAN_MAX {
        return Ok(Vec::new());
    }
    let idxs: Vec<u64> = (0..n).collect();
    let pairs = stream::iter(idxs)
        .map(|i| {
            let provider = provider.clone();
            async move {
                let call = IUniswapV2Factory::allPairsCall {
                    index: U256::from(i),
                };
                let raw = provider
                    .call(
                        alloy::rpc::types::TransactionRequest::default()
                            .to(factory)
                            .input(call.abi_encode().into()),
                    )
                    .await
                    .ok()?;
                IUniswapV2Factory::allPairsCall::abi_decode_returns(&raw).ok()
            }
        })
        .buffer_unordered(V2_FACTORY_SCAN_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    Ok(pairs
        .into_iter()
        .flatten()
        .filter(|a| !a.is_zero())
        .collect())
}

async fn read_v2_position_if_held(
    provider: &impl Provider,
    pair: Address,
    owner: Address,
) -> Result<Option<V2LpPosition>, WalletError> {
    let bal = read_v2_lp_balance(provider, pair, owner).await?;
    if bal.is_zero() {
        return Ok(None);
    }
    let t0_raw = provider
        .call(
            alloy::rpc::types::TransactionRequest::default()
                .to(pair)
                .input(IUniswapV2Pair::token0Call {}.abi_encode().into()),
        )
        .await
        .map_err(|e| WalletError::NetworkError(format!("token0: {e}")))?;
    let t1_raw = provider
        .call(
            alloy::rpc::types::TransactionRequest::default()
                .to(pair)
                .input(IUniswapV2Pair::token1Call {}.abi_encode().into()),
        )
        .await
        .map_err(|e| WalletError::NetworkError(format!("token1: {e}")))?;
    let token0 = IUniswapV2Pair::token0Call::abi_decode_returns(&t0_raw)
        .map_err(|e| WalletError::NetworkError(format!("decode token0: {e}")))?;
    let token1 = IUniswapV2Pair::token1Call::abi_decode_returns(&t1_raw)
        .map_err(|e| WalletError::NetworkError(format!("decode token1: {e}")))?;

    let supply_raw = provider
        .call(
            alloy::rpc::types::TransactionRequest::default()
                .to(pair)
                .input(IUniswapV2Pair::totalSupplyCall {}.abi_encode().into()),
        )
        .await
        .map_err(|e| WalletError::NetworkError(format!("totalSupply: {e}")))?;
    let total_supply = IUniswapV2Pair::totalSupplyCall::abi_decode_returns(&supply_raw)
        .map_err(|e| WalletError::NetworkError(format!("decode totalSupply: {e}")))?;

    let res_raw = provider
        .call(
            alloy::rpc::types::TransactionRequest::default()
                .to(pair)
                .input(IUniswapV2Pair::getReservesCall {}.abi_encode().into()),
        )
        .await
        .map_err(|e| WalletError::NetworkError(format!("getReserves: {e}")))?;
    let reserves = IUniswapV2Pair::getReservesCall::abi_decode_returns(&res_raw)
        .map_err(|e| WalletError::NetworkError(format!("decode getReserves: {e}")))?;

    Ok(Some(V2LpPosition {
        pair,
        token0,
        token1,
        lp_balance: bal,
        reserve0: U256::from(reserves.reserve0),
        reserve1: U256::from(reserves.reserve1),
        total_supply,
    }))
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

/// Transfer V2 LP ERC-20 shares from the pair contract to another wallet.
pub fn build_v2_transfer_lp_evm(
    from: &str,
    chain_id: u64,
    pair: Address,
    recipient: &str,
    amount: U256,
) -> Result<EvmTransaction, WalletError> {
    if pair.is_zero() {
        return Err(WalletError::InvalidTransaction(
            "pair address cannot be zero".into(),
        ));
    }
    if amount.is_zero() {
        return Err(WalletError::InvalidTransaction(
            "transfer amount must be > 0".into(),
        ));
    }
    let to = Address::from_str(recipient.trim())
        .map_err(|_| WalletError::InvalidTransaction("invalid recipient address".into()))?;
    if to == Address::ZERO {
        return Err(WalletError::InvalidTransaction(
            "recipient cannot be the zero address".into(),
        ));
    }
    let owner = Address::from_str(from.trim())
        .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?;
    if to == owner {
        return Err(WalletError::InvalidTransaction(
            "recipient is already the LP holder".into(),
        ));
    }
    let service = crate::core::transaction::TransactionService::new();
    let tx = service.build_erc20_transfer(
        from,
        format!("{pair:#x}"),
        recipient,
        amount.to_string(),
        chain_id,
    )?;
    match tx {
        crate::chains::ChainTransaction::Evm(evm) => Ok(evm),
        _ => Err(WalletError::InvalidTransaction(
            "expected an EVM LP transfer".into(),
        )),
    }
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
    fn ethw_hex_watch_pairs() {
        let hex = Address::from_str("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39").unwrap();
        let wethw = Address::from_str("0x7Bf88d2c0e32dE92Cdaf2D43CcDC23e8EdfD5990").unwrap();
        let weth = Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap();
        let lfg = default_v2_watch_pairs(10_001, DexVenue::LfgSwap);
        assert!(lfg.contains(&sort_pair(hex, wethw)));
        assert!(lfg.contains(&sort_pair(hex, weth)));
        assert_eq!(
            default_v2_watch_pairs(10_001, DexVenue::PowSwap),
            vec![sort_pair(hex, weth)]
        );
        assert_eq!(
            default_v2_watch_pairs(10_001, DexVenue::UniHedron),
            vec![sort_pair(hex, weth)]
        );
        assert_eq!(
            default_v2_watch_pairs(10_001, DexVenue::UniWswap),
            vec![sort_pair(hex, weth)]
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

    #[test]
    fn underlying_and_share_match_full_pool() {
        let supply = U256::from(1_000u64);
        let lp = U256::from(1_000u64);
        let r0 = U256::from(50_000u64);
        let r1 = U256::from(25_000u64);
        assert_eq!(v2_pool_share_bps(lp, supply), 10_000);
        assert_eq!(v2_underlying_amounts(lp, supply, r0, r1), (r0, r1));
    }

    #[test]
    fn half_share_halves_underlying() {
        let supply = U256::from(1_000u64);
        let lp = U256::from(500u64);
        let r0 = U256::from(100u64);
        let r1 = U256::from(200u64);
        assert_eq!(v2_pool_share_bps(lp, supply), 5_000);
        assert_eq!(
            v2_underlying_amounts(lp, supply, r0, r1),
            (U256::from(50u64), U256::from(100u64))
        );
    }

    #[test]
    fn zero_supply_is_safe() {
        assert_eq!(v2_pool_share_bps(U256::from(1u64), U256::ZERO), 0);
        assert_eq!(
            v2_underlying_amounts(
                U256::from(1u64),
                U256::ZERO,
                U256::from(9u64),
                U256::from(9u64)
            ),
            (U256::ZERO, U256::ZERO)
        );
        assert!(v2_spot_token1_per_token0(U256::ZERO, U256::from(1u64), 18, 18).is_none());
    }

    #[test]
    fn spot_price_one_to_two() {
        // 1e18 reserve0, 2e18 reserve1, both 18 dec → 2 token1 per token0
        let r0 = U256::from(10u64).pow(U256::from(18u64));
        let r1 = U256::from(2u64) * r0;
        let p = v2_spot_token1_per_token0(r0, r1, 18, 18).unwrap();
        assert_eq!(p, "2");
    }

    #[test]
    fn transfer_lp_rejects_bad_recipient() {
        let pair = Address::from_str("0x5b9F077A77db37F3Be0A5b5d31BAeff4bc5C0bD7").unwrap();
        let from = "0x0000000000000000000000000000000000000001";
        let err = build_v2_transfer_lp_evm(from, 369, pair, from, U256::from(1u64)).unwrap_err();
        assert!(err.user_message().contains("already"));
        let err = build_v2_transfer_lp_evm(
            from,
            369,
            pair,
            "0x0000000000000000000000000000000000000000",
            U256::from(1u64),
        )
        .unwrap_err();
        assert!(err.user_message().contains("zero"));
    }
}
