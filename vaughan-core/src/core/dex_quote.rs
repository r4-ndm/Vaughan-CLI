//! Direct DEX swap quotes for the TUI (read-only, no signing).
//!
//! - **V3 (943):** local exact-in math via [`wiz4rd-sdk`] on the wiz4rd deploy.
//! - **V3 (369+):** venue `QuoterV2` via `eth_call` when catalogued (9mm today).
//!   Multi-hop paths use Uniswap V3 packed `token | fee | token | …` bytes.
//! - **V2:** `router.getAmountsOut` via `eth_call` (Uni V2–compatible routers).
//!
//! **Route discovery (943 auto-fee)** follows the Uniswap / MetaMask-family pattern:
//! simulate exact-in quotes across catalog fee tiers (and WPLS hops), pick the
//! path with the best `amountOut` for the user's size. Swap math comes from the
//! pinned `uniswap-v3-sdk` crate via [`wiz4rd-math`]; router ABI matches the
//! Pancake / wiz4rd SwapRouter (`exactInput` / `exactInputSingle`).

use alloy::primitives::aliases::{U160, U24};
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use std::time::{Duration, Instant};

use crate::core::wiz4rd::{deployment_for_chain, parse_addr, WIZ4RD_FEE_TIERS};
use crate::error::WalletError;
use wiz4rd_sdk::config::Config;
use wiz4rd_sdk::error::SdkError;
use wiz4rd_sdk::pool::{get_pool_info, PoolInfo};
use wiz4rd_sdk::pool_address::get_pool_key;
use wiz4rd_sdk::tx::swap::{apply_slippage, zero_for_one, BasisPoints};
use wiz4rd_sdk::{quote_exact_in, Quote};

sol! {
    interface IUniswapV2RouterQuote {
        function getAmountsOut(uint256 amountIn, address[] calldata path)
            external
            view
            returns (uint256[] memory amounts);
    }

    interface IQuoterV2 {
        struct QuoteExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amountIn;
            uint24 fee;
            uint160 sqrtPriceLimitX96;
        }

        function quoteExactInputSingle(QuoteExactInputSingleParams memory params)
            external
            returns (
                uint256 amountOut,
                uint160 sqrtPriceX96After,
                uint32 initializedTicksCrossed,
                uint256 gasEstimate
            );

        function quoteExactInput(bytes memory path, uint256 amountIn)
            external
            returns (
                uint256 amountOut,
                uint160[] memory sqrtPriceX96AfterList,
                uint32[] memory initializedTicksCrossedList,
                uint256 gasEstimate
            );
    }
}

/// Normalized exact-in quote for Dex / CLI (output token raw units).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexQuote {
    pub amount_out: U256,
    /// Hop list used for the quote (matches swap calldata).
    pub path: Vec<Address>,
    /// One fee tier per hop (`path.len() - 1`); empty for V2.
    pub hop_fees: Vec<u32>,
    /// First-hop fee tier for display (`0` for V2).
    pub fee_tier: u32,
}

/// Best V3 swap route for a token pair (auto fee-tier + path selection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3DiscoveredRoute {
    pub path: Vec<Address>,
    /// Fee tier per hop (`path.len() - 1`).
    pub hop_fees: Vec<u32>,
    pub amount_out: U256,
}

/// Default slippage for auto min-out (0.5%).
pub const DEFAULT_DEX_SLIPPAGE_BPS: BasisPoints = 50;

/// Minimum output after slippage tolerance (exact-in swaps).
pub fn min_out_after_slippage(amount_out: U256, slippage_bps: BasisPoints) -> U256 {
    apply_slippage(amount_out, slippage_bps)
}

fn connect_http(rpc_url: &str) -> Result<impl Provider + use<>, WalletError> {
    let url = rpc_url
        .trim()
        .parse()
        .map_err(|e| WalletError::NetworkError(format!("invalid RPC URL: {e}")))?;
    Ok(ProviderBuilder::new().connect_http(url))
}

fn wiz4rd_config(chain_id: u64, rpc_url: &str) -> Result<Config, WalletError> {
    let dep = deployment_for_chain(chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "V3 pool quote is not wired for chain {chain_id} yet (wiz4rd deploy required)"
        ))
    })?;
    Ok(Config {
        rpc_url: Some(rpc_url.trim().to_string()),
        chain_id,
        factory: parse_addr(dep.factory),
        swap_router: parse_addr(dep.swap_router),
        position_manager: parse_addr(dep.position_manager),
        protocol_fee: 0,
        vaughan_provider: None,
        vaughan_origin: None,
    })
}

/// Single-hop V3 exact-in quote.
pub async fn quote_v3_exact_in(
    rpc_url: &str,
    chain_id: u64,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    fee: u32,
    quoter: Option<Address>,
) -> Result<DexQuote, WalletError> {
    quote_v3_path_exact_in(
        rpc_url,
        chain_id,
        &[token_in, token_out],
        amount_in,
        fee,
        quoter,
    )
    .await
}

/// Multi-hop V3 exact-in quote.
///
/// On wiz4rd chains (943) uses local pool math. Else uses `quoter` when set
/// (catalogued venue QuoterV2 on mainnet). Uses the same fee tier for every hop.
pub async fn quote_v3_path_exact_in(
    rpc_url: &str,
    chain_id: u64,
    path: &[Address],
    amount_in: U256,
    fee: u32,
    quoter: Option<Address>,
) -> Result<DexQuote, WalletError> {
    if path.len() < 2 {
        return Err(WalletError::InvalidTransaction(
            "V3 path must contain at least two tokens".into(),
        ));
    }
    if amount_in.is_zero() {
        let hop_fees = vec![fee; path.len().saturating_sub(1)];
        return Ok(DexQuote {
            amount_out: U256::ZERO,
            path: path.to_vec(),
            hop_fees,
            fee_tier: fee,
        });
    }

    if deployment_for_chain(chain_id).is_some() {
        quote_v3_path_wiz4rd_local(rpc_url, chain_id, path, amount_in, fee).await
    } else if let Some(q) = quoter {
        quote_v3_path_via_quoter(rpc_url, q, path, amount_in, fee).await
    } else {
        Err(WalletError::Other(format!(
            "V3 quote needs wiz4rd deploy or a catalogued QuoterV2 on chain {chain_id}"
        )))
    }
}

/// Simulate exact-in quotes across fee tiers and pick the best `amountOut`.
///
/// On wiz4rd (943) tries direct pools and independent per-hop fees on WPLS
/// routes. Requires a non-zero `amount_in` so quotes reflect trade size.
pub async fn discover_v3_swap_route(
    rpc_url: &str,
    chain_id: u64,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    wpls: Option<Address>,
    native_in: bool,
) -> Result<V3DiscoveredRoute, WalletError> {
    if amount_in.is_zero() {
        return Err(WalletError::InvalidAmount(
            "route discovery needs amount > 0".into(),
        ));
    }
    if native_in {
        let w = wpls.ok_or_else(|| {
            WalletError::InvalidTransaction(
                "native→token needs WPLS on PulseChain — switch network".into(),
            )
        })?;
        if token_out == w {
            return Err(WalletError::InvalidTransaction(
                "token out cannot be WPLS for native→token".into(),
            ));
        }
        return discover_v3_single_hop_route(rpc_url, chain_id, w, token_out, amount_in).await;
    }
    if token_in == token_out {
        return Err(WalletError::InvalidTransaction(
            "token in and token out must differ".into(),
        ));
    }

    let cfg = wiz4rd_config(chain_id, rpc_url)?;
    let provider = connect_http(rpc_url)?;
    let mut best: Option<V3DiscoveredRoute> = None;

    for &fee in WIZ4RD_FEE_TIERS {
        if let Some(out) =
            v3_quote_hop(&provider, &cfg, token_in, token_out, fee, amount_in).await?
        {
            consider_quote(&mut best, out, vec![token_in, token_out], vec![fee]);
        }
    }

    if let Some(w) = wpls {
        if token_in != w && token_out != w {
            for &fee_ab in WIZ4RD_FEE_TIERS {
                let mid =
                    match v3_quote_hop(&provider, &cfg, token_in, w, fee_ab, amount_in).await? {
                        Some(m) if !m.is_zero() => m,
                        _ => continue,
                    };
                for &fee_bc in WIZ4RD_FEE_TIERS {
                    if let Some(out) =
                        v3_quote_hop(&provider, &cfg, w, token_out, fee_bc, mid).await?
                    {
                        consider_quote(
                            &mut best,
                            out,
                            vec![token_in, w, token_out],
                            vec![fee_ab, fee_bc],
                        );
                    }
                }
            }
        }
    }

    best.ok_or_else(|| {
        WalletError::Other(format!(
            "no swappable V3 pool for {token_in:#x} ↔ {token_out:#x} — add LP or pick another pair"
        ))
    })
}

/// Prefer higher `amount_out`; on tie prefer fewer hops (direct over WPLS).
fn consider_quote(
    best: &mut Option<V3DiscoveredRoute>,
    amount_out: U256,
    path: Vec<Address>,
    hop_fees: Vec<u32>,
) {
    let route = V3DiscoveredRoute {
        path,
        hop_fees,
        amount_out,
    };
    match best {
        Some(prev) if prev.amount_out > amount_out => {}
        Some(prev) if prev.amount_out == amount_out && prev.path.len() <= route.path.len() => {}
        _ => *best = Some(route),
    }
}

async fn discover_v3_single_hop_route(
    rpc_url: &str,
    chain_id: u64,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
) -> Result<V3DiscoveredRoute, WalletError> {
    let cfg = wiz4rd_config(chain_id, rpc_url)?;
    let provider = connect_http(rpc_url)?;
    let mut best: Option<V3DiscoveredRoute> = None;
    for &fee in WIZ4RD_FEE_TIERS {
        if let Some(out) =
            v3_quote_hop(&provider, &cfg, token_in, token_out, fee, amount_in).await?
        {
            consider_quote(&mut best, out, vec![token_in, token_out], vec![fee]);
        }
    }
    best.ok_or_else(|| {
        WalletError::Other(format!(
            "no swappable V3 pool for {token_in:#x} → {token_out:#x}"
        ))
    })
}

async fn v3_quote_hop(
    provider: &impl Provider,
    cfg: &Config,
    token_in: Address,
    token_out: Address,
    fee: u32,
    amount_in: U256,
) -> Result<Option<U256>, WalletError> {
    let pool = match v3_load_pool_with(provider, cfg, token_in, token_out, fee).await? {
        Some(p) => p,
        None => return Ok(None),
    };
    Ok(Some(quote_v3_hop_exact_in(&pool, token_in, amount_in)?))
}

fn map_pool_read_error(e: SdkError) -> WalletError {
    match e {
        SdkError::PoolNotFound => {
            WalletError::Other("no V3 pool for this pair and fee tier".into())
        }
        e => WalletError::NetworkError(format!("pool read: {e}")),
    }
}

async fn v3_load_pool_with(
    provider: &impl Provider,
    cfg: &Config,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> Result<Option<PoolInfo>, WalletError> {
    let key = get_pool_key(token_a, token_b, fee);
    let pool = match get_pool_info(provider, cfg, key).await {
        Ok(p) => p,
        Err(SdkError::PoolNotFound) => return Ok(None),
        Err(e) => return Err(map_pool_read_error(e)),
    };
    if pool.liquidity == 0 || pool.sqrt_price_x96.is_zero() {
        return Ok(None);
    }
    Ok(Some(pool))
}

async fn v3_load_pool(
    rpc_url: &str,
    chain_id: u64,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> Result<Option<PoolInfo>, WalletError> {
    if deployment_for_chain(chain_id).is_none() {
        return Ok(None);
    }
    let cfg = wiz4rd_config(chain_id, rpc_url)?;
    let provider = connect_http(rpc_url)?;
    v3_load_pool_with(&provider, &cfg, token_a, token_b, fee).await
}

/// Resolve a V3 swap hop list: prefer a direct pool at `fee`, else WPLS routing.
pub async fn resolve_v3_swap_path(
    rpc_url: &str,
    chain_id: u64,
    token_in: Address,
    token_out: Address,
    fee: u32,
    wpls: Option<Address>,
    native_in: bool,
) -> Result<Vec<Address>, WalletError> {
    if native_in {
        let w = wpls.ok_or_else(|| {
            WalletError::InvalidTransaction(
                "native→token needs WPLS on PulseChain — switch network".into(),
            )
        })?;
        if token_out == w {
            return Err(WalletError::InvalidTransaction(
                "token out cannot be WPLS for native→token".into(),
            ));
        }
        return Ok(vec![w, token_out]);
    }
    if token_in == token_out {
        return Err(WalletError::InvalidTransaction(
            "token in and token out must differ".into(),
        ));
    }

    if v3_pool_exists(rpc_url, chain_id, token_in, token_out, fee).await? {
        return Ok(vec![token_in, token_out]);
    }

    if let Some(w) = wpls {
        if token_in != w && token_out != w {
            let via_wpls = v3_pool_exists(rpc_url, chain_id, token_in, w, fee).await?
                && v3_pool_exists(rpc_url, chain_id, w, token_out, fee).await?;
            if via_wpls {
                return Ok(vec![token_in, w, token_out]);
            }
        }
    }

    Err(WalletError::Other(format!(
        "no V3 pool for {token_in:#x} ↔ {token_out:#x} at fee {fee}"
    )))
}

async fn v3_pool_exists(
    rpc_url: &str,
    chain_id: u64,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> Result<bool, WalletError> {
    Ok(v3_load_pool(rpc_url, chain_id, token_a, token_b, fee)
        .await?
        .is_some())
}

async fn quote_v3_path_wiz4rd_local(
    rpc_url: &str,
    chain_id: u64,
    path: &[Address],
    amount_in: U256,
    fee: u32,
) -> Result<DexQuote, WalletError> {
    let cfg = wiz4rd_config(chain_id, rpc_url)?;
    let provider = connect_http(rpc_url)?;

    let mut amount = amount_in;
    for window in path.windows(2) {
        let token_in = window[0];
        let token_out = window[1];
        if token_in == token_out {
            return Err(WalletError::InvalidTransaction(
                "adjacent path tokens must differ".into(),
            ));
        }

        let key = get_pool_key(token_in, token_out, fee);
        let pool = get_pool_info(&provider, &cfg, key)
            .await
            .map_err(map_pool_read_error)?;
        if pool.pool.is_zero() {
            return Err(WalletError::Other(format!(
                "no V3 pool for {token_in:#x} → {token_out:#x} at fee {fee}"
            )));
        }

        amount = quote_v3_hop_exact_in(&pool, token_in, amount)?;
    }

    Ok(DexQuote {
        amount_out: amount,
        path: path.to_vec(),
        hop_fees: vec![fee; path.len() - 1],
        fee_tier: fee,
    })
}

/// Uniswap V3 packed path: `token (20) || fee (3) || token (20) || …`.
pub fn encode_v3_packed_path(tokens: &[Address], hop_fees: &[u32]) -> Result<Bytes, WalletError> {
    if tokens.len() < 2 {
        return Err(WalletError::InvalidTransaction(
            "V3 path needs at least two tokens".into(),
        ));
    }
    if hop_fees.len() != tokens.len() - 1 {
        return Err(WalletError::InvalidTransaction(
            "hop_fees length must equal path hops".into(),
        ));
    }
    let mut out = Vec::with_capacity(tokens.len() * 20 + (tokens.len() - 1) * 3);
    for (i, token) in tokens.iter().enumerate() {
        out.extend_from_slice(token.as_slice());
        if i + 1 < tokens.len() {
            let fee = hop_fees[i];
            if fee > 0xFF_FFFF {
                return Err(WalletError::InvalidTransaction(
                    "fee tier out of uint24 range".into(),
                ));
            }
            out.push(((fee >> 16) & 0xff) as u8);
            out.push(((fee >> 8) & 0xff) as u8);
            out.push((fee & 0xff) as u8);
        }
    }
    Ok(Bytes::from(out))
}

fn encode_v3_path(tokens: &[Address], fee: u32) -> Result<Bytes, WalletError> {
    encode_v3_packed_path(tokens, &vec![fee; tokens.len() - 1])
}

async fn quote_v3_path_via_quoter(
    rpc_url: &str,
    quoter: Address,
    path: &[Address],
    amount_in: U256,
    fee: u32,
) -> Result<DexQuote, WalletError> {
    let provider = connect_http(rpc_url)?;
    let raw = if path.len() == 2 {
        let fee_u24 = U24::try_from(fee)
            .map_err(|e| WalletError::InvalidTransaction(format!("bad V3 fee tier: {e}")))?;
        let call = IQuoterV2::quoteExactInputSingleCall {
            params: IQuoterV2::QuoteExactInputSingleParams {
                tokenIn: path[0],
                tokenOut: path[1],
                amountIn: amount_in,
                fee: fee_u24,
                sqrtPriceLimitX96: U160::ZERO,
            },
        };
        let tx = TransactionRequest::default()
            .to(quoter)
            .input(call.abi_encode().into());
        provider.call(tx).await.map_err(|e| {
            WalletError::NetworkError(format!("QuoterV2 quoteExactInputSingle: {e}"))
        })?
    } else {
        let path_bytes = encode_v3_path(path, fee)?;
        let call = IQuoterV2::quoteExactInputCall {
            path: path_bytes,
            amountIn: amount_in,
        };
        let tx = TransactionRequest::default()
            .to(quoter)
            .input(call.abi_encode().into());
        provider
            .call(tx)
            .await
            .map_err(|e| WalletError::NetworkError(format!("QuoterV2 quoteExactInput: {e}")))?
    };

    let amount_out = if path.len() == 2 {
        IQuoterV2::quoteExactInputSingleCall::abi_decode_returns(&raw)
            .map_err(|e| WalletError::NetworkError(format!("decode QuoterV2 single: {e}")))?
            .amountOut
    } else {
        IQuoterV2::quoteExactInputCall::abi_decode_returns(&raw)
            .map_err(|e| WalletError::NetworkError(format!("decode QuoterV2 path: {e}")))?
            .amountOut
    };

    Ok(DexQuote {
        amount_out,
        path: path.to_vec(),
        hop_fees: vec![fee; path.len() - 1],
        fee_tier: fee,
    })
}

const ALLOWANCE_WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const ALLOWANCE_WAIT_POLL: Duration = Duration::from_millis(750);

/// Whether `spender` already has enough ERC-20 allowance for an exact-in swap.
pub async fn erc20_allowance_covers(
    rpc_url: &str,
    token: Address,
    owner: Address,
    spender: Address,
    need: U256,
) -> Result<bool, WalletError> {
    use wiz4rd_sdk::allowance::get_allowance;

    if need.is_zero() {
        return Ok(true);
    }
    let provider = connect_http(rpc_url)?;
    let cur = get_allowance(&provider, token, owner, spender)
        .await
        .map_err(|e| WalletError::NetworkError(format!("allowance: {e}")))?;
    Ok(cur >= need)
}

/// Poll ERC-20 `allowance` until `spender` can pull at least `need` (post-approve Dex step).
pub async fn wait_erc20_allowance(
    rpc_url: &str,
    token: Address,
    owner: Address,
    spender: Address,
    need: U256,
) -> Result<(), WalletError> {
    use wiz4rd_sdk::allowance::get_allowance;

    if need.is_zero() {
        return Ok(());
    }
    let provider = connect_http(rpc_url)?;
    let start = Instant::now();
    loop {
        let cur = get_allowance(&provider, token, owner, spender)
            .await
            .map_err(|e| WalletError::NetworkError(format!("allowance: {e}")))?;
        if cur >= need {
            return Ok(());
        }
        if start.elapsed() >= ALLOWANCE_WAIT_TIMEOUT {
            return Err(WalletError::NetworkError(
                "approve not confirmed within 60s — wait for the block, then press F4 again".into(),
            ));
        }
        tokio::time::sleep(ALLOWANCE_WAIT_POLL).await;
    }
}

fn quote_v3_hop_exact_in(
    pool: &PoolInfo,
    token_in: Address,
    amount_in: U256,
) -> Result<U256, WalletError> {
    if amount_in.is_zero() {
        return Ok(U256::ZERO);
    }
    let zfo = zero_for_one(pool, token_in);
    if token_in != pool.token0 && token_in != pool.token1 {
        return Err(WalletError::InvalidTransaction(
            "token in is not in this pool".into(),
        ));
    }
    let Quote { amount_out, .. } = quote_exact_in(pool, amount_in, zfo)
        .map_err(|e| WalletError::Other(format!("V3 quote math: {e}")))?;
    Ok(amount_out)
}

/// V2 exact-in quote via router `getAmountsOut`.
pub async fn quote_v2_exact_in(
    rpc_url: &str,
    router: Address,
    amount_in: U256,
    path: &[Address],
) -> Result<DexQuote, WalletError> {
    if amount_in.is_zero() {
        return Ok(DexQuote {
            amount_out: U256::ZERO,
            path: path.to_vec(),
            hop_fees: vec![],
            fee_tier: 0,
        });
    }
    if path.len() < 2 {
        return Err(WalletError::InvalidTransaction(
            "swap path must have at least two tokens".into(),
        ));
    }

    let provider = connect_http(rpc_url)?;
    let call = IUniswapV2RouterQuote::getAmountsOutCall {
        amountIn: amount_in,
        path: path.to_vec(),
    };
    let tx = TransactionRequest::default()
        .to(router)
        .input(call.abi_encode().into());
    let raw = provider
        .call(tx)
        .await
        .map_err(|e| WalletError::NetworkError(format!("getAmountsOut: {e}")))?;
    let amounts = IUniswapV2RouterQuote::getAmountsOutCall::abi_decode_returns(&raw)
        .map_err(|e| WalletError::NetworkError(format!("decode getAmountsOut: {e}")))?;
    let amount_out = amounts
        .last()
        .copied()
        .ok_or_else(|| WalletError::NetworkError("getAmountsOut returned empty amounts".into()))?;
    Ok(DexQuote {
        amount_out,
        path: path.to_vec(),
        hop_fees: vec![],
        fee_tier: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;
    use wiz4rd_sdk::pool_address::PoolKey;

    fn pool_with(
        token0: Address,
        token1: Address,
        liquidity: u128,
        tick: i32,
        fee: u32,
    ) -> PoolInfo {
        use alloy::primitives::aliases::I24;
        use alloy::primitives::aliases::U160;
        let sqrt: U160 = wiz4rd_math::get_sqrt_ratio_at_tick(I24::try_from(tick).unwrap()).unwrap();
        let limbs = sqrt.into_limbs();
        let sqrt_price_x96 = U256::from_limbs([limbs[0], limbs[1], limbs[2], 0]);
        PoolInfo {
            pool_key: PoolKey {
                token0,
                token1,
                fee,
            },
            pool: Address::repeat_byte(0x99),
            token0,
            token1,
            fee,
            sqrt_price_x96,
            tick,
            fee_protocol: 0,
            liquidity,
        }
    }

    #[test]
    fn min_out_slippage_rounds_down() {
        let out = U256::from(1_000_000u64);
        assert_eq!(
            min_out_after_slippage(out, DEFAULT_DEX_SLIPPAGE_BPS),
            U256::from(995_000u64)
        );
    }

    #[test]
    fn v3_path_chains_hops_locally() {
        let token_a = Address::repeat_byte(0x11);
        let token_b = Address::repeat_byte(0x22);
        let token_c = Address::repeat_byte(0x33);
        let liq = 10u128.pow(22);
        let pool_ab = pool_with(token_a, token_b, liq, 0, 500);
        let pool_bc = pool_with(token_b, token_c, liq, 0, 500);
        let amount_in = U256::from(10u128.pow(17));

        let one_hop = quote_v3_hop_exact_in(&pool_ab, token_a, amount_in).unwrap();
        assert!(one_hop > U256::ZERO);

        let two_hop = quote_v3_hop_exact_in(&pool_bc, token_b, one_hop).unwrap();
        assert!(two_hop > U256::ZERO);
        assert!(
            two_hop < one_hop,
            "second hop should reduce output vs mid token"
        );
    }

    #[test]
    fn consider_quote_picks_best_output() {
        let a = Address::repeat_byte(0x11);
        let b = Address::repeat_byte(0x22);
        let mut best: Option<V3DiscoveredRoute> = None;
        consider_quote(&mut best, U256::from(100u64), vec![a, b], vec![500]);
        consider_quote(&mut best, U256::from(200u64), vec![a, b], vec![20_000]);
        let route = best.unwrap();
        assert_eq!(route.hop_fees, vec![20_000]);
        assert_eq!(route.amount_out, U256::from(200u64));
    }

    #[test]
    fn consider_quote_prefers_shorter_path_on_tie() {
        let a = Address::repeat_byte(0x11);
        let b = Address::repeat_byte(0x22);
        let w = Address::repeat_byte(0xaa);
        let mut best: Option<V3DiscoveredRoute> = None;
        consider_quote(&mut best, U256::from(100u64), vec![a, w, b], vec![500, 500]);
        consider_quote(&mut best, U256::from(100u64), vec![a, b], vec![20_000]);
        assert_eq!(best.unwrap().path, vec![a, b]);
    }

    #[tokio::test]
    async fn v3_path_rejects_short_path() {
        let err = quote_v3_path_exact_in(
            "http://127.0.0.1:1",
            943,
            &[Address::repeat_byte(0x01)],
            U256::from(1u64),
            500,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, WalletError::InvalidTransaction(_)));
    }

    #[tokio::test]
    #[ignore = "live PulseChain testnet 943 RPC"]
    async fn live_discover_bob_jane_943() {
        use std::str::FromStr;
        let bob = Address::from_str("0x15de8ae884726f37ec90824f825d723ac93c8b77").unwrap();
        let jane = Address::from_str("0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59").unwrap();
        let wpls = Address::from_str("0x70499adEBB11Efd915E3b69E700c331778628707").unwrap();
        let route = discover_v3_swap_route(
            "https://rpc.v4.testnet.pulsechain.com",
            943,
            bob,
            jane,
            U256::from(1_000u64) * U256::from(10u128.pow(18)),
            Some(wpls),
            false,
        )
        .await
        .expect("BOB→JANE should discover the 2% pool");
        assert_eq!(route.hop_fees, vec![20_000]);
        assert_eq!(route.path, vec![bob, jane]);
        assert!(route.amount_out > U256::ZERO);
    }

    #[tokio::test]
    #[ignore = "live PulseChain testnet 943 RPC"]
    async fn live_discover_wpls_wzrd_943() {
        use std::str::FromStr;
        let wpls = Address::from_str("0x70499adEBB11Efd915E3b69E700c331778628707").unwrap();
        let wzrd = Address::from_str("0x29bab93456c0E97EE931C1554c7C215480aa7766").unwrap();
        let route = discover_v3_swap_route(
            "https://rpc.v4.testnet.pulsechain.com",
            943,
            wpls,
            wzrd,
            U256::from(10u64) * U256::from(10u128.pow(18)),
            Some(wpls),
            true,
        )
        .await
        .expect("native/WPLS→WZRD");
        assert_eq!(route.hop_fees, vec![500]);
        assert_eq!(route.path, vec![wpls, wzrd]);
        assert!(route.amount_out > U256::ZERO);
    }

    #[tokio::test]
    async fn discover_errors_when_no_pool() {
        let err = discover_v3_swap_route(
            "http://127.0.0.1:1",
            943,
            Address::repeat_byte(0x11),
            Address::repeat_byte(0x22),
            U256::from(1_000u64),
            Some(Address::repeat_byte(0xaa)),
            false,
        )
        .await
        .unwrap_err();
        assert!(!matches!(err, WalletError::InvalidTransaction(_)));
    }

    #[test]
    fn encode_v3_packed_path_per_hop_fees() {
        let a = Address::repeat_byte(0x11);
        let b = Address::repeat_byte(0x22);
        let c = Address::repeat_byte(0x33);
        let packed = encode_v3_packed_path(&[a, b, c], &[500, 20_000]).unwrap();
        assert_eq!(packed.len(), 66);
        assert_eq!(&packed[20..23], &[0, 0x01, 0xf4]); // 500
        assert_eq!(&packed[43..46], &[0, 0x4e, 0x20]); // 20000
    }
}
