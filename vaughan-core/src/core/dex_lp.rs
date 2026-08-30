//! V3 concentrated liquidity (NPM) — browserless position reads + tx build.
//!
//! Wraps [`wiz4rd-sdk`] liquidity builders for the TUI (same contracts as MCP
//! `propose_v3_*`). Venues resolve NPM + factory from [`super::dex_catalog`]
//! (wiz4rd 943, 9mm 369 today).

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::chains::EvmTransaction;
use crate::core::dex_catalog::{
    venue_position_manager, venue_swap_router, venue_v3_factory, DexProtocol, DexVenue,
};
use crate::core::dex_routers::is_allowed_dex_router;
use crate::error::WalletError;
use wiz4rd_sdk::config::Config;
use wiz4rd_sdk::positions::{list_positions_from, PositionInfo};
use wiz4rd_sdk::tx::liquidity::{
    build_collect_tx, build_decrease_liquidity_tx, build_increase_liquidity_tx, build_mint_tx,
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

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
