//! V3 concentrated liquidity (NPM) — browserless position reads + tx build.
//!
//! Wraps [`wiz4rd-sdk`] liquidity builders for the TUI (same contracts as MCP
//! `propose_v3_*`). Venues resolve NPM + factory from [`super::dex_catalog`]
//! (wiz4rd 943, 9mm 369 today).

use alloy::primitives::{Address, U160, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::chains::EvmTransaction;
use crate::core::dex_catalog::{
    venue_position_manager, venue_swap_router, venue_v3_factory, DexProtocol, DexVenue,
};
use crate::core::dex_quote::{min_out_after_slippage, DEFAULT_DEX_SLIPPAGE_BPS};
use crate::core::dex_routers::is_allowed_dex_router;
use crate::core::transaction::parse_native_amount;
use crate::error::WalletError;
use wiz4rd_sdk::abi::IPancakeV3Factory;
use wiz4rd_sdk::config::Config;
use wiz4rd_sdk::pool::{get_pool_info, PoolInfo};
use wiz4rd_sdk::pool_address::get_pool_key;
use wiz4rd_sdk::positions::{list_positions_from, PositionInfo};
use wiz4rd_sdk::tx::liquidity::{
    build_collect_tx, build_decrease_liquidity_tx, build_increase_liquidity_tx, build_mint_tx,
};
use wiz4rd_sdk::tx::pool::{build_create_pool_tx, build_initialize_pool_tx};

/// Re-export for TUI / CLI display.
pub use wiz4rd_sdk::positions::PositionInfo as V3PositionInfo;

/// Build wiz4rd-sdk config for a catalogued V3 LP venue on `chain_id`.
pub fn v3_lp_sdk_config(
    venue: DexVenue,
    chain_id: u64,
    rpc_url: &str,
) -> Result<Config, WalletError> {
    let npm = venue_position_manager(venue, chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 NPM on chain {chain_id}",
            venue.label()
        ))
    })?;
    assert_npm_allowed(chain_id, npm)?;
    let factory = venue_v3_factory(venue, chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 factory on chain {chain_id}",
            venue.label()
        ))
    })?;
    let swap_router = venue_swap_router(venue, DexProtocol::V3, chain_id);
    Ok(Config {
        rpc_url: Some(rpc_url.trim().to_string()),
        chain_id,
        factory: Some(factory),
        swap_router,
        position_manager: Some(npm),
        protocol_fee: 0,
        vaughan_provider: None,
        vaughan_origin: None,
    })
}

/// Back-compat alias for wiz4rd-only callers.
pub fn wiz4rd_sdk_config(chain_id: u64, rpc_url: &str) -> Result<Config, WalletError> {
    v3_lp_sdk_config(DexVenue::Wiz4rd, chain_id, rpc_url)
}

fn connect_http(rpc_url: &str) -> Result<impl Provider + use<>, WalletError> {
    let url = rpc_url
        .trim()
        .parse()
        .map_err(|e| WalletError::NetworkError(format!("invalid RPC URL: {e}")))?;
    Ok(ProviderBuilder::new().connect_http(url))
}

fn default_deadline_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_add(1200))
        .unwrap_or(0)
}

fn assert_npm_allowed(chain_id: u64, npm: Address) -> Result<(), WalletError> {
    if !is_allowed_dex_router(chain_id, npm) {
        return Err(WalletError::InvalidTransaction(format!(
            "position manager {npm:#x} is not allowlisted on chain {chain_id}"
        )));
    }
    Ok(())
}

fn assert_factory_allowed(chain_id: u64, factory: Address) -> Result<(), WalletError> {
    if !is_allowed_dex_router(chain_id, factory) {
        return Err(WalletError::InvalidTransaction(format!(
            "V3 factory {factory:#x} is not allowlisted on chain {chain_id}"
        )));
    }
    Ok(())
}

/// On-chain pool context for V3 mint deposit preview (TUI / agents).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3LpPoolQuote {
    pub lifecycle: V3PoolLifecycle,
    /// Live `slot0.sqrtPriceX96` when [`V3PoolLifecycle::Ready`].
    pub sqrt_price_x96: Option<U160>,
    /// Live `slot0.tick` when [`V3PoolLifecycle::Ready`].
    pub tick: Option<i32>,
    /// Human token1-per-token0 price when the pool is initialized.
    pub pool_price_token1_per_token0: Option<String>,
}

/// V3 pool deployment stage for a token pair + fee tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V3PoolLifecycle {
    /// `factory.getPool` returned zero — needs `createPool`.
    Missing,
    /// Pool contract exists but price not set — needs `initialize`.
    Uninitialized { pool: Address },
    /// Pool exists and has a non-zero sqrt price — ready for mint / swap.
    Ready,
}

async fn factory_get_pool(
    provider: &impl Provider,
    factory: Address,
    key: wiz4rd_sdk::pool_address::PoolKey,
) -> Result<Address, WalletError> {
    use alloy::primitives::aliases::U24;
    let call = IPancakeV3Factory::getPoolCall {
        tokenA: key.token0,
        tokenB: key.token1,
        fee: U24::try_from(key.fee)
            .map_err(|e| WalletError::NetworkError(format!("fee tier: {e}")))?,
    };
    let raw = provider
        .call(
            TransactionRequest::default()
                .to(factory)
                .input(call.abi_encode().into()),
        )
        .await
        .map_err(|e| WalletError::NetworkError(format!("getPool: {e}")))?;
    IPancakeV3Factory::getPoolCall::abi_decode_returns(&raw)
        .map_err(|e| WalletError::NetworkError(format!("decode getPool: {e}")))
}

/// Resolve whether a V3 pool needs create, initialize, or is ready to mint.
pub async fn v3_pool_lifecycle(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> Result<V3PoolLifecycle, WalletError> {
    let factory = venue_v3_factory(venue, chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 factory on chain {chain_id}",
            venue.label()
        ))
    })?;
    let key = get_pool_key(token_a, token_b, fee);
    let provider = connect_http(rpc_url)?;
    let pool = factory_get_pool(&provider, factory, key).await?;
    if pool.is_zero() {
        return Ok(V3PoolLifecycle::Missing);
    }
    match get_pool_info(&provider, &v3_lp_sdk_config(venue, chain_id, rpc_url)?, key).await {
        Ok(info) if info.sqrt_price_x96.is_zero() => Ok(V3PoolLifecycle::Uninitialized { pool }),
        Ok(_) => Ok(V3PoolLifecycle::Ready),
        Err(_) => Ok(V3PoolLifecycle::Uninitialized { pool }),
    }
}

/// Optional on-chain wait before resolving the next deploy tx (multi-step LP).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum V3LpDeployWait {
    #[default]
    None,
    /// Poll factory `getPool` after a `createPool` broadcast.
    AfterCreatePool,
    /// Poll pool `sqrtPriceX96` after an `initialize` broadcast.
    AfterInitialize,
    /// Poll NPM `allowance` after an ERC-20 `approve` broadcast.
    AfterApprove,
}

const DEPLOY_WAIT_POLL: Duration = Duration::from_secs(2);
const DEPLOY_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Block until a prior deploy step is visible on-chain (or timeout).
pub async fn v3_lp_run_deploy_wait(
    wait: V3LpDeployWait,
    params: &V3LpDeployParams,
) -> Result<(), WalletError> {
    match wait {
        V3LpDeployWait::None => Ok(()),
        V3LpDeployWait::AfterCreatePool => {
            v3_lp_wait_for_pool_address(
                &params.rpc_url,
                params.venue,
                params.chain_id,
                params.token0,
                params.token1,
                params.fee,
            )
            .await
        }
        V3LpDeployWait::AfterInitialize => {
            v3_lp_wait_for_pool_initialized(
                &params.rpc_url,
                params.venue,
                params.chain_id,
                params.token0,
                params.token1,
                params.fee,
            )
            .await
        }
        V3LpDeployWait::AfterApprove => v3_lp_wait_for_mint_allowances(params).await,
    }
}

async fn v3_lp_wait_for_pool_address(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    token0: Address,
    token1: Address,
    fee: u32,
) -> Result<(), WalletError> {
    let factory = venue_v3_factory(venue, chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 factory on chain {chain_id}",
            venue.label()
        ))
    })?;
    let key = get_pool_key(token0, token1, fee);
    let provider = connect_http(rpc_url)?;
    let start = Instant::now();
    loop {
        let pool = factory_get_pool(&provider, factory, key).await?;
        if !pool.is_zero() {
            return Ok(());
        }
        if start.elapsed() >= DEPLOY_WAIT_TIMEOUT {
            return Err(WalletError::NetworkError(
                "createPool not confirmed within 60s — wait for the block, then retry".into(),
            ));
        }
        tokio::time::sleep(DEPLOY_WAIT_POLL).await;
    }
}

async fn v3_lp_wait_for_pool_initialized(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    token0: Address,
    token1: Address,
    fee: u32,
) -> Result<(), WalletError> {
    let start = Instant::now();
    loop {
        match v3_pool_lifecycle(rpc_url, venue, chain_id, token0, token1, fee).await? {
            V3PoolLifecycle::Ready => return Ok(()),
            V3PoolLifecycle::Uninitialized { pool } => {
                let cfg = v3_lp_sdk_config(venue, chain_id, rpc_url)?;
                let provider = connect_http(rpc_url)?;
                let key = get_pool_key(token0, token1, fee);
                if let Ok(info) = get_pool_info(&provider, &cfg, key).await {
                    if !info.sqrt_price_x96.is_zero() {
                        return Ok(());
                    }
                }
                let _ = pool;
            }
            V3PoolLifecycle::Missing => {}
        }
        if start.elapsed() >= DEPLOY_WAIT_TIMEOUT {
            return Err(WalletError::NetworkError(
                "initialize not confirmed within 60s — wait for the block, then retry".into(),
            ));
        }
        tokio::time::sleep(DEPLOY_WAIT_POLL).await;
    }
}

/// Wait until at least one mint `approve` is reflected on-chain (avoids duplicate approve loops).
async fn v3_lp_wait_for_mint_allowances(params: &V3LpDeployParams) -> Result<(), WalletError> {
    use wiz4rd_sdk::allowance::get_allowance;

    let npm = venue_position_manager(params.venue, params.chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 NPM on chain {}",
            params.venue.label(),
            params.chain_id
        ))
    })?;
    let owner = Address::from_str(params.from.trim())
        .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?;
    let amount0_wei = parse_deposit_wei(&params.amount0, params.dec0, "amount0")?;
    let (need0, need1) = v3_lp_mint_amounts_at_pool(params, amount0_wei).await?;
    let provider = connect_http(&params.rpc_url)?;
    let start = Instant::now();
    loop {
        let cur0 = get_allowance(&provider, params.token0, owner, npm)
            .await
            .map_err(|e| WalletError::NetworkError(format!("allowance: {e}")))?;
        let cur1 = get_allowance(&provider, params.token1, owner, npm)
            .await
            .map_err(|e| WalletError::NetworkError(format!("allowance: {e}")))?;
        if cur0 >= need0 && cur1 >= need1 {
            return Ok(());
        }
        // One approve confirmed — next deploy step may queue the other token or mint.
        if cur0 >= need0 || cur1 >= need1 {
            return Ok(());
        }
        if start.elapsed() >= DEPLOY_WAIT_TIMEOUT {
            return Err(WalletError::NetworkError(
                "approve not confirmed within 60s — wait for the block, then retry".into(),
            ));
        }
        tokio::time::sleep(DEPLOY_WAIT_POLL).await;
    }
}

/// Inputs for one step of the V3 create → initialize → approve → mint pipeline.
pub struct V3LpDeployParams {
    pub from: String,
    pub venue: DexVenue,
    pub chain_id: u64,
    pub rpc_url: String,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub dec0: u8,
    pub dec1: u8,
    pub pool_initial_price: String,
    pub pool_min_price: String,
    pub pool_max_price: String,
    pub amount0: String,
    pub amount1: String,
}

fn parse_deposit_wei(raw: &str, decimals: u8, label: &str) -> Result<U256, WalletError> {
    let wei_str = parse_native_amount(raw.trim(), decimals)?;
    U256::from_str(&wei_str).map_err(|_| WalletError::InvalidAmount(format!("invalid {label}")))
}

fn v3_lp_range_ticks(params: &V3LpDeployParams) -> Result<(i32, i32), WalletError> {
    if params.pool_min_price.trim().is_empty() || params.pool_max_price.trim().is_empty() {
        default_full_range_ticks(params.fee)
    } else {
        v3_range_ticks_from_human_prices(
            params.chain_id,
            params.token0,
            params.token1,
            params.dec0,
            params.dec1,
            params.pool_min_price.trim(),
            params.pool_max_price.trim(),
            params.fee,
        )
    }
}

/// Mint amounts coupled to live pool price (token0 deposit is the anchor).
#[allow(clippy::too_many_arguments)]
async fn v3_lp_mint_amounts_at_pool(
    params: &V3LpDeployParams,
    amount0_wei: U256,
) -> Result<(U256, U256), WalletError> {
    let (_, info) = load_v3_lp_pool(
        &params.rpc_url,
        params.venue,
        params.chain_id,
        params.token0,
        params.token1,
        params.fee,
    )
    .await?;
    let sqrt = sqrt_price_u160(info.sqrt_price_x96)?;
    v3_preview_mint_deposits_from_amount0(
        params.chain_id,
        params.token0,
        params.token1,
        params.dec0,
        params.dec1,
        params.fee,
        sqrt,
        info.tick,
        &params.pool_min_price,
        &params.pool_max_price,
        amount0_wei,
    )
}

#[allow(clippy::too_many_arguments)]
async fn v3_lp_first_needed_approve(
    provider: &impl Provider,
    from: &str,
    chain_id: u64,
    npm: Address,
    token0: Address,
    token1: Address,
    need0: U256,
    need1: U256,
) -> Result<Option<(EvmTransaction, String)>, WalletError> {
    use wiz4rd_sdk::allowance::ensure_allowance_txs;

    let owner = Address::from_str(from.trim())
        .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?;
    for (token, need, name) in [(token0, need0, "token0"), (token1, need1, "token1")] {
        if need.is_zero() {
            continue;
        }
        let txs = ensure_allowance_txs(provider, token, owner, npm, need)
            .await
            .map_err(|e| WalletError::NetworkError(format!("allowance: {e}")))?;
        if let Some(req) = txs.first() {
            let label = if txs.len() > 1 {
                format!("approve {name} for LP (step 1/2: reset)")
            } else {
                format!("approve {name} for LP")
            };
            return Ok(Some((tx_to_evm(from, chain_id, req.clone())?, label)));
        }
    }
    Ok(None)
}

/// Next on-chain tx for V3 pool deploy / add-LP (one step per call).
pub async fn v3_lp_prepare_deploy_step(
    params: &V3LpDeployParams,
) -> Result<(EvmTransaction, String), WalletError> {
    if params.token0 >= params.token1 {
        return Err(WalletError::InvalidTransaction(
            "token0 must be sorted below token1".into(),
        ));
    }
    let lifecycle = v3_pool_lifecycle(
        &params.rpc_url,
        params.venue,
        params.chain_id,
        params.token0,
        params.token1,
        params.fee,
    )
    .await?;
    match lifecycle {
        V3PoolLifecycle::Missing => {
            // createPool may have mined between scans — avoid a duplicate that reverts.
            let factory = venue_v3_factory(params.venue, params.chain_id).ok_or_else(|| {
                WalletError::Other(format!(
                    "{} has no V3 factory on chain {}",
                    params.venue.label(),
                    params.chain_id
                ))
            })?;
            let provider = connect_http(&params.rpc_url)?;
            let pool = factory_get_pool(
                &provider,
                factory,
                get_pool_key(params.token0, params.token1, params.fee),
            )
            .await?;
            if !pool.is_zero() {
                return Box::pin(v3_lp_prepare_deploy_step(params)).await;
            }
            build_v3_create_pool_evm(
                &params.from,
                params.venue,
                params.chain_id,
                &params.rpc_url,
                params.token0,
                params.token1,
                params.fee,
            )
            .map(|tx| (tx, "createPool".to_string()))
        }
        V3PoolLifecycle::Uninitialized { pool } => {
            let init_price = if params.pool_initial_price.trim().is_empty() {
                params.pool_min_price.trim()
            } else {
                params.pool_initial_price.trim()
            };
            if init_price.is_empty() {
                return Err(WalletError::InvalidTransaction(
                    "set initial price for new pool".into(),
                ));
            }
            build_v3_initialize_pool_from_human_price_evm(
                &params.from,
                params.chain_id,
                pool,
                params.token0,
                params.token1,
                params.dec0,
                params.dec1,
                init_price,
                params.fee,
            )
            .map(|tx| (tx, "initialize".to_string()))
        }
        V3PoolLifecycle::Ready => {
            let amount0_wei = parse_deposit_wei(&params.amount0, params.dec0, "amount0")?;
            if amount0_wei.is_zero() {
                return Err(WalletError::InvalidTransaction(
                    "deposit amount0 must be > 0".into(),
                ));
            }
            let (amount0, amount1) = v3_lp_mint_amounts_at_pool(params, amount0_wei).await?;
            if amount1.is_zero() {
                return Err(WalletError::InvalidTransaction(
                    "deposit amounts must be > 0 for this range".into(),
                ));
            }
            let npm = venue_position_manager(params.venue, params.chain_id).ok_or_else(|| {
                WalletError::Other(format!(
                    "{} has no V3 NPM on chain {}",
                    params.venue.label(),
                    params.chain_id
                ))
            })?;
            let provider = connect_http(&params.rpc_url)?;
            if let Some((tx, label)) = v3_lp_first_needed_approve(
                &provider,
                &params.from,
                params.chain_id,
                npm,
                params.token0,
                params.token1,
                amount0,
                amount1,
            )
            .await?
            {
                return Ok((tx, label));
            }
            let (tick_lower, tick_upper) = v3_lp_range_ticks(params)?;
            let amount0_min = min_out_after_slippage(amount0, DEFAULT_DEX_SLIPPAGE_BPS);
            let amount1_min = min_out_after_slippage(amount1, DEFAULT_DEX_SLIPPAGE_BPS);
            build_v3_mint_evm(
                &params.from,
                params.venue,
                params.chain_id,
                &params.rpc_url,
                params.token0,
                params.token1,
                params.fee,
                tick_lower,
                tick_upper,
                amount0,
                amount1,
                amount0_min,
                amount1_min,
                None,
            )
            .map(|tx| (tx, "add liquidity".to_string()))
        }
    }
}

/// Map an initial price tick to `sqrtPriceX96` (Pancake / Uni V3 TickMath).
pub fn sqrt_price_x96_from_tick(tick: i32) -> Result<U160, WalletError> {
    use alloy::primitives::aliases::I24;
    use wiz4rd_math::get_sqrt_ratio_at_tick;
    get_sqrt_ratio_at_tick(
        I24::try_from(tick)
            .map_err(|e| WalletError::InvalidTransaction(format!("invalid tick: {e}")))?,
    )
    .map_err(|e| WalletError::InvalidTransaction(format!("tick math: {e}")))
}

/// Factory `createPool` for a catalogued V3 venue.
pub fn build_v3_create_pool_evm(
    from: &str,
    venue: DexVenue,
    chain_id: u64,
    rpc_url: &str,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> Result<EvmTransaction, WalletError> {
    let factory = venue_v3_factory(venue, chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 factory on chain {chain_id}",
            venue.label()
        ))
    })?;
    assert_factory_allowed(chain_id, factory)?;
    let cfg = v3_lp_sdk_config(venue, chain_id, rpc_url)?;
    let req = build_create_pool_tx(&cfg, token_a, token_b, fee)
        .map_err(|e| WalletError::InvalidTransaction(format!("createPool calldata: {e}")))?;
    tx_to_evm(from, chain_id, req)
}

/// Pool `initialize(sqrtPriceX96)` on `pool` (must be factory.getPool for the pair).
pub fn build_v3_initialize_pool_evm(
    from: &str,
    chain_id: u64,
    pool: Address,
    sqrt_price_x96: U160,
) -> Result<EvmTransaction, WalletError> {
    if pool.is_zero() {
        return Err(WalletError::InvalidTransaction(
            "pool address must be non-zero".into(),
        ));
    }
    let req = build_initialize_pool_tx(pool, sqrt_price_x96)
        .map_err(|e| WalletError::InvalidTransaction(format!("initialize calldata: {e}")))?;
    tx_to_evm(from, chain_id, req)
}

/// Initialize using `initial_tick` (0 = 1:1 token1/token0 in raw ratio space).
pub fn build_v3_initialize_pool_from_tick_evm(
    from: &str,
    chain_id: u64,
    pool: Address,
    initial_tick: i32,
) -> Result<EvmTransaction, WalletError> {
    let sqrt = sqrt_price_x96_from_tick(initial_tick)?;
    build_v3_initialize_pool_evm(from, chain_id, pool, sqrt)
}

fn tx_to_evm(
    from: &str,
    chain_id: u64,
    req: TransactionRequest,
) -> Result<EvmTransaction, WalletError> {
    let to = req
        .to
        .as_ref()
        .and_then(|t| t.to().copied())
        .ok_or_else(|| WalletError::InvalidTransaction("LP tx missing to address".into()))?;
    let data = req.input.input().map(|b| format!("0x{}", hex::encode(b)));
    Ok(EvmTransaction {
        from: from.to_string(),
        to: format!("{to:#x}"),
        value: req.value.unwrap_or_default().to_string(),
        data,
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id,
    })
}

/// Load on-chain pool state for a catalogued V3 LP venue (mint preview / tick range).
fn sqrt_price_u160(sqrt: U256) -> Result<U160, WalletError> {
    let bytes = sqrt.to_be_bytes::<32>();
    if bytes[..12].iter().any(|&b| b != 0) {
        return Err(WalletError::InvalidTransaction(
            "pool sqrtPriceX96 does not fit U160".into(),
        ));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes[12..32]);
    Ok(U160::from_be_bytes(out))
}

#[allow(clippy::too_many_arguments)]
/// Fetch pool lifecycle + live price for V3 add-LP deposit preview.
pub async fn fetch_v3_lp_pool_quote(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    fee: u32,
) -> Result<V3LpPoolQuote, WalletError> {
    if token0 >= token1 {
        return Err(WalletError::InvalidTransaction(
            "fetch_v3_lp_pool_quote requires token0 < token1".into(),
        ));
    }
    let lifecycle = v3_pool_lifecycle(rpc_url, venue, chain_id, token0, token1, fee).await?;
    match lifecycle {
        V3PoolLifecycle::Ready => {
            let (_, info) = load_v3_lp_pool(rpc_url, venue, chain_id, token0, token1, fee).await?;
            let sqrt = sqrt_price_u160(info.sqrt_price_x96)?;
            let human = wiz4rd_math::pool_tick_to_human_price(
                chain_id, token0, token1, dec0, dec1, info.tick,
            )
            .map_err(WalletError::InvalidTransaction)?;
            Ok(V3LpPoolQuote {
                lifecycle: V3PoolLifecycle::Ready,
                sqrt_price_x96: Some(sqrt),
                tick: Some(info.tick),
                pool_price_token1_per_token0: Some(human),
            })
        }
        other => Ok(V3LpPoolQuote {
            lifecycle: other,
            sqrt_price_x96: None,
            tick: None,
            pool_price_token1_per_token0: None,
        }),
    }
}

/// Resolve sqrt price + tick from on-chain pool state or a human starting price.
#[allow(clippy::too_many_arguments)]
pub fn v3_sqrt_and_tick_for_preview(
    chain_id: u64,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    fee: u32,
    pool_sqrt: Option<U160>,
    pool_tick: Option<i32>,
    fallback_human_price_token1_per_token0: &str,
) -> Result<(U160, i32), WalletError> {
    if let (Some(sqrt), Some(tick)) = (pool_sqrt, pool_tick) {
        return Ok((sqrt, tick));
    }
    let price = fallback_human_price_token1_per_token0.trim();
    if price.is_empty() {
        return Err(WalletError::InvalidTransaction(
            "set current price for deposit preview".into(),
        ));
    }
    let tick = v3_initial_tick_from_human_price(chain_id, token0, token1, dec0, dec1, price, fee)?;
    let sqrt = sqrt_price_x96_from_tick(tick)?;
    Ok((sqrt, tick))
}

/// Pancake-style mint amounts from a token0 deposit (`Position::from_amount0` + `mint_amounts`).
#[allow(clippy::too_many_arguments)]
pub fn v3_preview_mint_deposits_from_amount0(
    chain_id: u64,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    fee: u32,
    sqrt_price_x96: U160,
    tick_current: i32,
    min_price_token1_per_token0: &str,
    max_price_token1_per_token0: &str,
    amount0_wei: U256,
) -> Result<(U256, U256), WalletError> {
    let (tick_lower, tick_upper) = if min_price_token1_per_token0.trim().is_empty()
        || max_price_token1_per_token0.trim().is_empty()
    {
        default_full_range_ticks(fee)?
    } else {
        v3_range_ticks_from_human_prices(
            chain_id,
            token0,
            token1,
            dec0,
            dec1,
            min_price_token1_per_token0.trim(),
            max_price_token1_per_token0.trim(),
            fee,
        )?
    };
    let amounts = wiz4rd_math::v3_mint_amounts_from_amount0(
        sqrt_price_x96,
        tick_current,
        tick_lower,
        tick_upper,
        amount0_wei,
    )
    .map_err(WalletError::InvalidTransaction)?;
    Ok((amounts.amount0, amounts.amount1))
}

/// Pancake-style mint amounts from a token1 deposit (`Position::from_amount1` + `mint_amounts`).
#[allow(clippy::too_many_arguments)]
pub fn v3_preview_mint_deposits_from_amount1(
    chain_id: u64,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    fee: u32,
    sqrt_price_x96: U160,
    tick_current: i32,
    min_price_token1_per_token0: &str,
    max_price_token1_per_token0: &str,
    amount1_wei: U256,
) -> Result<(U256, U256), WalletError> {
    let (tick_lower, tick_upper) = if min_price_token1_per_token0.trim().is_empty()
        || max_price_token1_per_token0.trim().is_empty()
    {
        default_full_range_ticks(fee)?
    } else {
        v3_range_ticks_from_human_prices(
            chain_id,
            token0,
            token1,
            dec0,
            dec1,
            min_price_token1_per_token0.trim(),
            max_price_token1_per_token0.trim(),
            fee,
        )?
    };
    let amounts = wiz4rd_math::v3_mint_amounts_from_amount1(
        sqrt_price_x96,
        tick_current,
        tick_lower,
        tick_upper,
        amount1_wei,
    )
    .map_err(WalletError::InvalidTransaction)?;
    Ok((amounts.amount0, amounts.amount1))
}

pub async fn load_v3_lp_pool(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> Result<(Config, PoolInfo), WalletError> {
    let cfg = v3_lp_sdk_config(venue, chain_id, rpc_url)?;
    let key = get_pool_key(token_a, token_b, fee);
    let provider = connect_http(rpc_url)?;
    let info = get_pool_info(&provider, &cfg, key)
        .await
        .map_err(|e| WalletError::NetworkError(format!("get_pool_info: {e}")))?;
    if info.pool.is_zero() {
        return Err(WalletError::NetworkError(
            "pool does not exist for this pair/fee".into(),
        ));
    }
    Ok((cfg, info))
}

/// List V3 LP NFT positions for `owner` (Transfer-log scan + `positions()`).
pub async fn list_v3_lp_positions(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    owner: Address,
    from_block: Option<u64>,
    to_block: Option<u64>,
) -> Result<Vec<PositionInfo>, WalletError> {
    let cfg = v3_lp_sdk_config(venue, chain_id, rpc_url)?;
    let provider = connect_http(rpc_url)?;
    list_positions_from(&provider, &cfg, owner, from_block, to_block)
        .await
        .map_err(|e| WalletError::NetworkError(format!("list positions: {e}")))
}

/// Open a new concentrated LP position (mint NFT).
#[allow(clippy::too_many_arguments)]
pub fn build_v3_mint_evm(
    from: &str,
    venue: DexVenue,
    chain_id: u64,
    rpc_url: &str,
    token0: Address,
    token1: Address,
    fee: u32,
    tick_lower: i32,
    tick_upper: i32,
    amount0_desired: U256,
    amount1_desired: U256,
    amount0_min: U256,
    amount1_min: U256,
    deadline: Option<u64>,
) -> Result<EvmTransaction, WalletError> {
    let cfg = v3_lp_sdk_config(venue, chain_id, rpc_url)?;
    if token0 >= token1 {
        return Err(WalletError::InvalidTransaction(format!(
            "V3 mint requires token0 < token1 (got {token0:#x}, {token1:#x})"
        )));
    }
    let recipient = Address::from_str(from)
        .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?;
    let req = build_mint_tx(
        &cfg,
        token0,
        token1,
        fee,
        tick_lower,
        tick_upper,
        amount0_desired,
        amount1_desired,
        amount0_min,
        amount1_min,
        recipient,
        deadline.unwrap_or_else(default_deadline_secs),
    )
    .map_err(|e| WalletError::InvalidTransaction(format!("mint calldata: {e}")))?;
    tx_to_evm(from, chain_id, req)
}

#[allow(clippy::too_many_arguments)]
pub fn build_v3_increase_evm(
    from: &str,
    venue: DexVenue,
    chain_id: u64,
    rpc_url: &str,
    token_id: U256,
    amount0_desired: U256,
    amount1_desired: U256,
    amount0_min: U256,
    amount1_min: U256,
    deadline: Option<u64>,
) -> Result<EvmTransaction, WalletError> {
    let cfg = v3_lp_sdk_config(venue, chain_id, rpc_url)?;
    let req = build_increase_liquidity_tx(
        &cfg,
        token_id,
        amount0_desired,
        amount1_desired,
        amount0_min,
        amount1_min,
        deadline.unwrap_or_else(default_deadline_secs),
    )
    .map_err(|e| WalletError::InvalidTransaction(format!("increase calldata: {e}")))?;
    tx_to_evm(from, chain_id, req)
}

#[allow(clippy::too_many_arguments)]
pub fn build_v3_decrease_evm(
    from: &str,
    venue: DexVenue,
    chain_id: u64,
    rpc_url: &str,
    token_id: U256,
    liquidity: u128,
    amount0_min: U256,
    amount1_min: U256,
    deadline: Option<u64>,
) -> Result<EvmTransaction, WalletError> {
    let cfg = v3_lp_sdk_config(venue, chain_id, rpc_url)?;
    let req = build_decrease_liquidity_tx(
        &cfg,
        token_id,
        liquidity,
        amount0_min,
        amount1_min,
        deadline.unwrap_or_else(default_deadline_secs),
    )
    .map_err(|e| WalletError::InvalidTransaction(format!("decrease calldata: {e}")))?;
    tx_to_evm(from, chain_id, req)
}

#[allow(clippy::too_many_arguments)]
pub fn build_v3_collect_evm(
    from: &str,
    venue: DexVenue,
    chain_id: u64,
    rpc_url: &str,
    token_id: U256,
    recipient: Option<Address>,
    amount0_max: u128,
    amount1_max: u128,
) -> Result<EvmTransaction, WalletError> {
    let cfg = v3_lp_sdk_config(venue, chain_id, rpc_url)?;
    let payee = match recipient {
        Some(a) => a,
        None => Address::from_str(from)
            .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?,
    };
    let req = build_collect_tx(&cfg, token_id, payee, amount0_max, amount1_max)
        .map_err(|e| WalletError::InvalidTransaction(format!("collect calldata: {e}")))?;
    tx_to_evm(from, chain_id, req)
}

pub use wiz4rd_math::display_price_range_from_preset;

/// Full-range concentrated LP ticks for `fee` (smoke / first mint).
pub fn default_full_range_ticks(fee: u32) -> Result<(i32, i32), WalletError> {
    use wiz4rd_math::fee_tiers::tick_spacing;
    use wiz4rd_math::nearest_usable_tick;
    let spacing = tick_spacing(fee)
        .ok_or_else(|| WalletError::InvalidTransaction(format!("unsupported V3 fee tier {fee}")))?;
    Ok((
        nearest_usable_tick(-887272, spacing),
        nearest_usable_tick(887272, spacing),
    ))
}

/// Human min/max prices (token1 per token0) → mint tick range.
#[allow(clippy::too_many_arguments)]
pub fn v3_range_ticks_from_human_prices(
    chain_id: u64,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    min_price: &str,
    max_price: &str,
    fee: u32,
) -> Result<(i32, i32), WalletError> {
    wiz4rd_math::pool_price_range_to_usable_ticks(
        chain_id, token0, token1, dec0, dec1, min_price, max_price, fee,
    )
    .map_err(WalletError::InvalidTransaction)
}

/// Initialize tick from human pool price (token1 per token0).
#[allow(clippy::too_many_arguments)]
pub fn v3_initial_tick_from_human_price(
    chain_id: u64,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    price_token1_per_token0: &str,
    fee: u32,
) -> Result<i32, WalletError> {
    wiz4rd_math::pool_price_to_usable_tick(
        chain_id,
        token0,
        token1,
        dec0,
        dec1,
        price_token1_per_token0,
        fee,
    )
    .map_err(WalletError::InvalidTransaction)
}

/// Pool `initialize` from a human starting price (token1 per token0).
#[allow(clippy::too_many_arguments)]
pub fn build_v3_initialize_pool_from_human_price_evm(
    from: &str,
    chain_id: u64,
    pool: Address,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    price_token1_per_token0: &str,
    fee: u32,
) -> Result<EvmTransaction, WalletError> {
    let tick = v3_initial_tick_from_human_price(
        chain_id,
        token0,
        token1,
        dec0,
        dec1,
        price_token1_per_token0,
        fee,
    )?;
    build_v3_initialize_pool_from_tick_evm(from, chain_id, pool, tick)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, U160};

    #[test]
    fn tick_zero_sqrt_for_initialize() {
        let sqrt = sqrt_price_x96_from_tick(0).unwrap();
        assert_eq!(sqrt, U160::from(1u128 << 96));
    }

    #[test]
    fn sdk_config_for_wiz4rd_943_and_nine_mm_369() {
        assert!(v3_lp_sdk_config(DexVenue::Wiz4rd, 943, "http://127.0.0.1:8545").is_ok());
        assert!(v3_lp_sdk_config(DexVenue::NineMm, 369, "http://127.0.0.1:8545").is_ok());
        assert!(v3_lp_sdk_config(DexVenue::NineMm, 943, "http://127.0.0.1:8545").is_err());
    }

    #[test]
    fn mint_rejects_venue_without_npm_on_chain() {
        let err = build_v3_mint_evm(
            "0x0000000000000000000000000000000000000001",
            DexVenue::NineMm,
            943,
            "http://127.0.0.1:8545",
            Address::ZERO,
            Address::ZERO,
            500,
            -887220,
            887220,
            U256::from(1u64),
            U256::from(1u64),
            U256::ZERO,
            U256::ZERO,
            None,
        )
        .unwrap_err();
        assert!(err.user_message().contains("9mm") || err.to_string().contains("943"));
    }

    #[test]
    fn mint_rejects_unsorted_token_pair() {
        let wzrd = address!("0x29bab93456c0E97EE931C1554c7C215480aa7766");
        let wpls = address!("0x70499adEBB11Efd915E3b69E700c331778628707");
        let err = build_v3_mint_evm(
            "0x0000000000000000000000000000000000000001",
            DexVenue::Wiz4rd,
            943,
            "http://127.0.0.1:8545",
            wpls,
            wzrd,
            500,
            -887220,
            887220,
            U256::from(1u64),
            U256::from(1u64),
            U256::ZERO,
            U256::ZERO,
            None,
        )
        .unwrap_err();
        assert!(matches!(err, WalletError::InvalidTransaction(_)));
        assert!(err.user_message().contains("token0 < token1"));
    }
}
