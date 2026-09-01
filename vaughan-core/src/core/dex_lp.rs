//! V3 concentrated liquidity (NPM) — browserless position reads + tx build.
//!
//! Wraps [`wiz4rd-sdk`] liquidity builders for the TUI (same contracts as MCP
//! `propose_v3_*`). Venues resolve NPM + factory from [`super::dex_catalog`]
//! (wiz4rd 943, 9mm 369 today).

use alloy::primitives::{Address, U160, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use serde::{Deserialize, Serialize};
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

/// Primary RPC first, then built-in fallbacks (deduped, non-empty).
pub fn merge_rpc_urls(primary: &str, fallbacks: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |u: &str| {
        let t = u.trim();
        if t.is_empty() {
            return;
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    };
    push(primary);
    for u in fallbacks {
        push(u);
    }
    out
}

/// True when another RPC endpoint may succeed (transport / HTTP / decode flake).
pub fn is_lp_rpc_transport(err: &WalletError) -> bool {
    match err {
        WalletError::NetworkError(m) => {
            m.starts_with("getPool:")
                || m.starts_with("get_pool_info:")
                || m.starts_with("decode getPool:")
                || m.starts_with("allowance:")
                || m.starts_with("block number:")
                || m.contains("invalid RPC URL")
                || m.contains("timed out")
                || m.contains("connection")
                || m.contains("connect")
                || m.contains("no RPC URL")
        }
        WalletError::RpcError(_) => true,
        _ => false,
    }
}

/// Run `call` against primary RPC, then network fallbacks on transport failures.
pub async fn with_lp_rpc_urls<T, F, Fut>(rpc_urls: &[String], mut call: F) -> Result<T, WalletError>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<T, WalletError>>,
{
    if rpc_urls.is_empty() {
        return Err(WalletError::NetworkError(
            "no RPC URL configured — set network RPC in Settings (F1)".into(),
        ));
    }
    let mut last: Option<WalletError> = None;
    for url in rpc_urls {
        match call(url.clone()).await {
            Ok(v) => return Ok(v),
            Err(e) if is_lp_rpc_transport(&e) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| WalletError::RpcError("all LP RPC endpoints failed".into())))
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
    /// When the requested fee had no pool but another tier does (TUI should switch ←→).
    pub suggested_fee_tier: Option<u32>,
}

/// Standard V3 fee tiers on Pulse / 9mm catalog venues.
pub const V3_LP_FEE_TIERS: [u32; 5] = [100, 500, 2500, 10_000, 20_000];

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// Optional context for the deploy wait (e.g. which approve tx was just broadcast).
#[derive(Clone, Debug, Default)]
pub struct V3LpDeployContext {
    pub last_step_label: Option<String>,
}

/// Block until a prior deploy step is visible on-chain (or timeout).
pub async fn v3_lp_run_deploy_wait(
    wait: V3LpDeployWait,
    params: &V3LpDeployParams,
    ctx: Option<&V3LpDeployContext>,
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
        V3LpDeployWait::AfterApprove => {
            v3_lp_wait_for_next_approve(params, ctx.and_then(|c| c.last_step_label.as_deref()))
                .await
        }
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

fn approve_label_token(label: &str) -> Option<&str> {
    label
        .strip_prefix("approve ")
        .and_then(|rest| rest.split_whitespace().next())
}

fn allowance_covers_mint(cur: U256, need: U256) -> bool {
    need.is_zero() || cur >= need || cur == U256::MAX
}

async fn read_allowance_once(
    provider: &impl Provider,
    token: Address,
    owner: Address,
    npm: Address,
) -> Result<U256, WalletError> {
    use wiz4rd_sdk::allowance::get_allowance;

    get_allowance(provider, token, owner, npm)
        .await
        .map_err(|e| WalletError::NetworkError(format!("allowance: {e}")))
}

/// Poll until two consecutive reads match (post-approve confirmation wait).
async fn read_allowance_stable(
    provider: &impl Provider,
    token: Address,
    owner: Address,
    npm: Address,
) -> Result<U256, WalletError> {
    let mut cur = read_allowance_once(provider, token, owner, npm).await?;
    for _ in 0..2 {
        tokio::time::sleep(DEPLOY_WAIT_POLL).await;
        let next = read_allowance_once(provider, token, owner, npm).await?;
        if next == cur {
            return Ok(cur);
        }
        cur = next;
    }
    Ok(cur)
}

/// Wait until the approve tx we just broadcast is reflected (reset → 0, enable → MAX/need).
async fn v3_lp_wait_for_next_approve(
    params: &V3LpDeployParams,
    last_label: Option<&str>,
) -> Result<(), WalletError> {
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
    let (need0, need1) = v3_lp_deploy_mint_amounts(params).await?;
    let provider = connect_http(&params.rpc_url)?;
    let start = Instant::now();

    let wait_reset = last_label.is_some_and(|l| l.contains("reset"));
    if wait_reset {
        let name = last_label.and_then(approve_label_token).unwrap_or("token0");
        let token = if name == "token1" {
            params.token1
        } else {
            params.token0
        };
        loop {
            let cur = get_allowance(&provider, token, owner, npm)
                .await
                .map_err(|e| WalletError::NetworkError(format!("allowance: {e}")))?;
            if cur.is_zero() {
                return Ok(());
            }
            if start.elapsed() >= DEPLOY_WAIT_TIMEOUT {
                return Err(WalletError::NetworkError(
                    "approve reset not confirmed within 60s — wait for the block, then retry"
                        .into(),
                ));
            }
            tokio::time::sleep(DEPLOY_WAIT_POLL).await;
        }
    }

    let wait_targets: &[(Address, U256)] = match last_label.and_then(approve_label_token) {
        Some("token1") => &[(params.token1, need1)],
        Some("token0") => &[(params.token0, need0)],
        _ => &[(params.token0, need0), (params.token1, need1)],
    };

    for &(token, need) in wait_targets {
        if need.is_zero() {
            continue;
        }
        let cur = read_allowance_stable(&provider, token, owner, npm).await?;
        if allowance_covers_mint(cur, need) {
            continue;
        }
        loop {
            let cur = get_allowance(&provider, token, owner, npm)
                .await
                .map_err(|e| WalletError::NetworkError(format!("allowance: {e}")))?;
            if allowance_covers_mint(cur, need) {
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
    Ok(())
}

/// Inputs for one step of the V3 create → initialize → approve → mint pipeline.
#[derive(Clone)]
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
    /// Which sorted leg holds the user's one-sided deposit (false = deposit on token1).
    pub deposit_on_token0: bool,
}

fn parse_deposit_wei(raw: &str, decimals: u8, label: &str) -> Result<U256, WalletError> {
    let wei_str = parse_native_amount(raw.trim(), decimals)?;
    U256::from_str(&wei_str).map_err(|_| WalletError::InvalidAmount(format!("invalid {label}")))
}

/// Tick range for a deploy/mint from human min/max or full-range preset.
pub fn v3_lp_mint_tick_range(params: &V3LpDeployParams) -> Result<(i32, i32), WalletError> {
    v3_lp_range_ticks(params)
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

/// Fix swapped human deposit legs (e.g. `amount0: "300"`, `amount1: "90.36…"` when deposit was 300 T2).
pub fn lp_deploy_fixup_swapped_amounts(params: &mut V3LpDeployParams) {
    let (deposit, other) = if params.deposit_on_token0 {
        (&mut params.amount0, &mut params.amount1)
    } else {
        (&mut params.amount1, &mut params.amount0)
    };
    if looks_like_computed_deposit(deposit.trim()) && looks_like_user_deposit(other.trim()) {
        std::mem::swap(&mut params.amount0, &mut params.amount1);
    }
}

fn looks_like_user_deposit(raw: &str) -> bool {
    let s = raw.trim();
    if s.is_empty() {
        return false;
    }
    match s.split_once('.') {
        None => true,
        Some((_, frac)) => frac.len() <= 2,
    }
}

fn looks_like_computed_deposit(raw: &str) -> bool {
    let s = raw.trim();
    s.split_once('.')
        .is_some_and(|(_, frac)| frac.len() > 4)
}

/// Mint deposit amounts in wei for deploy batching or preflight balance checks.
pub async fn v3_lp_deploy_mint_amounts(
    params: &V3LpDeployParams,
) -> Result<(U256, U256), WalletError> {
    let mut params = params.clone();
    lp_deploy_fixup_swapped_amounts(&mut params);
    let amount0_wei = parse_deposit_wei(&params.amount0, params.dec0, "amount0")?;
    let amount1_wei = parse_deposit_wei(&params.amount1, params.dec1, "amount1")?;
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
        V3PoolLifecycle::Ready => {
            if params.deposit_on_token0 {
                v3_lp_mint_amounts_at_pool(&params, amount0_wei).await
            } else {
                v3_lp_mint_amounts_at_pool_from_amount1(&params, amount1_wei).await
            }
        }
        V3PoolLifecycle::Missing | V3PoolLifecycle::Uninitialized { .. } => {
            Ok((amount0_wei, amount1_wei))
        }
    }
}

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
    let sqrt = v3_pool_sqrt_u160(info.sqrt_price_x96)?;
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

async fn v3_lp_mint_amounts_at_pool_from_amount1(
    params: &V3LpDeployParams,
    amount1_wei: U256,
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
    let sqrt = v3_pool_sqrt_u160(info.sqrt_price_x96)?;
    v3_preview_mint_deposits_from_amount1(
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
        amount1_wei,
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
    use wiz4rd_sdk::allowance::build_approve_tx;

    let owner = Address::from_str(from.trim())
        .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?;
    for (token, need, name) in [(token0, need0, "token0"), (token1, need1, "token1")] {
        if need.is_zero() {
            continue;
        }
        let current = read_allowance_once(provider, token, owner, npm).await?;
        if allowance_covers_mint(current, need) {
            continue;
        }
        // USDT-style: zero before a new approval when allowance is stuck non-zero.
        if !current.is_zero() {
            let req = build_approve_tx(token, npm, U256::ZERO);
            let label = format!("approve {name} for LP (reset)");
            return Ok(Some((tx_to_evm(from, chain_id, req)?, label)));
        }
        // PancakeSwap-style Enable: infinite NPM approval (avoids re-approve loops when mint need shifts).
        let req = build_approve_tx(token, npm, U256::MAX);
        let label = format!("approve {name} for LP");
        return Ok(Some((tx_to_evm(from, chain_id, req)?, label)));
    }
    Ok(None)
}

/// On-chain enable status for sorted `token0` / `token1` when the pool is [`V3PoolLifecycle::Ready`].
///
/// `None` when the pool is missing or uninitialized — enables happen after create/initialize.
pub async fn v3_lp_token_enable_status(
    params: &V3LpDeployParams,
) -> Result<Option<(bool, bool)>, WalletError> {
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
    if !matches!(lifecycle, V3PoolLifecycle::Ready) {
        return Ok(None);
    }
    let (need0, need1) = v3_lp_deploy_mint_amounts(params).await?;
    let npm = venue_position_manager(params.venue, params.chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 NPM on chain {}",
            params.venue.label(),
            params.chain_id
        ))
    })?;
    let owner = Address::from_str(params.from.trim())
        .map_err(|_| WalletError::InvalidTransaction("invalid from address".into()))?;
    let provider = connect_http(&params.rpc_url)?;
    let cur0 = read_allowance_once(&provider, params.token0, owner, npm).await?;
    let cur1 = read_allowance_once(&provider, params.token1, owner, npm).await?;
    Ok(Some((
        allowance_covers_mint(cur0, need0),
        allowance_covers_mint(cur1, need1),
    )))
}

/// Next PancakeSwap-style **Enable** tx for the NPM (reset or infinite approve).
pub async fn v3_lp_build_next_enable_tx(
    params: &V3LpDeployParams,
) -> Result<Option<(EvmTransaction, String)>, WalletError> {
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
    if !matches!(lifecycle, V3PoolLifecycle::Ready) {
        return Ok(None);
    }
    let (need0, need1) = v3_lp_deploy_mint_amounts(params).await?;
    let npm = venue_position_manager(params.venue, params.chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 NPM on chain {}",
            params.venue.label(),
            params.chain_id
        ))
    })?;
    let provider = connect_http(&params.rpc_url)?;
    v3_lp_first_needed_approve(
        &provider,
        &params.from,
        params.chain_id,
        npm,
        params.token0,
        params.token1,
        need0,
        need1,
    )
    .await
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
            let (amount0, amount1) = v3_lp_deploy_mint_amounts(params).await?;
            if amount0.is_zero() || amount1.is_zero() {
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

/// Convert pool `sqrtPriceX96` (U256 from slot0) to U160 for mint preview math.
pub fn v3_pool_sqrt_u160(sqrt: U256) -> Result<U160, WalletError> {
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
    let mut effective_fee = fee;
    let mut suggested_fee_tier = None;
    let first = v3_pool_lifecycle(rpc_url, venue, chain_id, token0, token1, effective_fee).await?;
    let lifecycle = if matches!(first, V3PoolLifecycle::Missing) {
        if let Some(found) =
            discover_v3_pool_fee_tier(rpc_url, venue, chain_id, token0, token1).await?
        {
            if found != effective_fee {
                suggested_fee_tier = Some(found);
                effective_fee = found;
                v3_pool_lifecycle(rpc_url, venue, chain_id, token0, token1, effective_fee).await?
            } else {
                first
            }
        } else {
            first
        }
    } else {
        first
    };
    match lifecycle {
        V3PoolLifecycle::Ready => {
            let (_, info) =
                load_v3_lp_pool(rpc_url, venue, chain_id, token0, token1, effective_fee).await?;
            let sqrt = v3_pool_sqrt_u160(info.sqrt_price_x96)?;
            let human = wiz4rd_math::pool_tick_to_human_price(
                chain_id, token0, token1, dec0, dec1, info.tick,
            )
            .map_err(WalletError::InvalidTransaction)?;
            Ok(V3LpPoolQuote {
                lifecycle: V3PoolLifecycle::Ready,
                sqrt_price_x96: Some(sqrt),
                tick: Some(info.tick),
                pool_price_token1_per_token0: Some(human),
                suggested_fee_tier,
            })
        }
        other => Ok(V3LpPoolQuote {
            lifecycle: other,
            sqrt_price_x96: None,
            tick: None,
            pool_price_token1_per_token0: None,
            suggested_fee_tier,
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

/// Mint preview from a token0 deposit when tick bounds are already known (matches NPM mint).
pub fn v3_preview_mint_deposits_from_amount0_ticks(
    sqrt_price_x96: U160,
    tick_current: i32,
    tick_lower: i32,
    tick_upper: i32,
    amount0_wei: U256,
) -> Result<(U256, U256), WalletError> {
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

/// Mint preview from a token1 deposit when tick bounds are already known (matches NPM mint).
pub fn v3_preview_mint_deposits_from_amount1_ticks(
    sqrt_price_x96: U160,
    tick_current: i32,
    tick_lower: i32,
    tick_upper: i32,
    amount1_wei: U256,
) -> Result<(U256, U256), WalletError> {
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

/// First catalog fee tier with an on-chain pool for `token0`/`token1` (any lifecycle except Missing).
pub async fn discover_v3_pool_fee_tier(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    token0: Address,
    token1: Address,
) -> Result<Option<u32>, WalletError> {
    if token0 >= token1 {
        return Err(WalletError::InvalidTransaction(
            "discover_v3_pool_fee_tier requires token0 < token1".into(),
        ));
    }
    let factory = venue_v3_factory(venue, chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 factory on chain {chain_id}",
            venue.label()
        ))
    })?;
    let provider = connect_http(rpc_url)?;
    for fee in V3_LP_FEE_TIERS {
        let pool = factory_get_pool(&provider, factory, get_pool_key(token0, token1, fee)).await?;
        if !pool.is_zero() {
            return Ok(Some(fee));
        }
    }
    Ok(None)
}

/// Block to start NPM `Transfer` log scans when the caller did not bound `from_block`.
fn lp_positions_scan_from_block(chain_id: u64, latest: u64) -> u64 {
    use crate::core::wiz4rd::NPM_LOG_SCAN_FROM_BLOCK_943;
    match chain_id {
        // Local anvil reuses chain id 943 with a tiny head — scan from genesis.
        943 if latest < NPM_LOG_SCAN_FROM_BLOCK_943 => 0,
        943 => NPM_LOG_SCAN_FROM_BLOCK_943,
        369 => latest.saturating_sub(500_000),
        _ => latest.saturating_sub(50_000),
    }
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
    let scan_from = match from_block {
        Some(b) => b,
        None => {
            let latest = provider
                .get_block_number()
                .await
                .map_err(|e| WalletError::NetworkError(format!("block number: {e}")))?;
            lp_positions_scan_from_block(chain_id, latest)
        }
    };
    list_positions_from(&provider, &cfg, owner, Some(scan_from), to_block)
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
    fn fixup_swaps_round_deposit_out_of_wrong_leg() {
        use alloy::primitives::address;
        let mut params = V3LpDeployParams {
            from: String::new(),
            venue: DexVenue::Wiz4rd,
            chain_id: 943,
            rpc_url: String::new(),
            token0: address!("0x33df366093ef8ac488e5be40e7ee2eeac2142770"),
            token1: address!("0xfc413180d3624349d111fd98ee76bc08a25bc655"),
            fee: 20000,
            dec0: 18,
            dec1: 18,
            pool_initial_price: String::new(),
            pool_min_price: String::new(),
            pool_max_price: String::new(),
            amount0: "300".into(),
            amount1: "90.363684870695".into(),
            deposit_on_token0: false,
        };
        lp_deploy_fixup_swapped_amounts(&mut params);
        assert_eq!(params.amount1, "300");
        assert_eq!(params.amount0, "90.363684870695");
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

    #[test]
    fn merge_rpc_urls_dedupes_primary_and_fallbacks() {
        let urls = merge_rpc_urls(
            "https://rpc.v4.testnet.pulsechain.com",
            &[
                "https://pulsechain-testnet-rpc.publicnode.com".into(),
                "https://rpc.v4.testnet.pulsechain.com".into(),
            ],
        );
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://rpc.v4.testnet.pulsechain.com");
    }

    #[test]
    fn lp_positions_scan_from_block_943_live_rpc() {
        use crate::core::wiz4rd::NPM_LOG_SCAN_FROM_BLOCK_943;
        assert_eq!(lp_positions_scan_from_block(943, 12), 0);
        assert_eq!(
            lp_positions_scan_from_block(943, NPM_LOG_SCAN_FROM_BLOCK_943),
            NPM_LOG_SCAN_FROM_BLOCK_943
        );
    }
}
