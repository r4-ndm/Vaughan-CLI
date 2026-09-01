//! Human-friendly LP deploy inputs ("Brews") and conversion to [`V3LpDeployParams`].
//!
//! Shared by the TUI LP view, MCP `propose_v3_lp_deploy`, and `vaughan lp` CLI.
//! Price/range helpers map user token order to on-chain `token0 < token1` order.

use std::path::Path;
use std::str::FromStr;

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};

use crate::core::dex_lp::{
    discover_v3_pool_fee_tier, v3_pool_lifecycle, v3_pool_sqrt_u160,
    v3_preview_mint_deposits_from_amount0, v3_preview_mint_deposits_from_amount1,
    v3_sqrt_and_tick_for_preview, V3LpDeployParams, V3PoolLifecycle,
};
use crate::core::dex_catalog::{default_lp_v3_venue, parse_dex_venue_label, DexVenue};
use crate::core::wiz4rd::{parse_addr, WPLS_943, WZRD_SMOKE_943};
use crate::core::transaction::{format_display_amount, parse_native_amount};
use crate::error::WalletError;

/// On-chain sorted token pair plus whether the user's "token A" is `token0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortedLpTokens {
    pub token0: Address,
    pub token1: Address,
    pub dec0: u8,
    pub dec1: u8,
    /// `true` when the user's first-named token equals on-chain `token0`.
    pub first_is_token0: bool,
}

/// Range selection for a Brew.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LpRangeInput {
    #[default]
    Full,
    MinMax {
        min: String,
        max: String,
    },
}

/// Human-facing LP deploy inputs before RPC resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LpHumanInputs {
    pub from: String,
    pub chain_id: u64,
    pub rpc_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
    /// First token as named by the user (symbol or `0x` address).
    pub token_a: String,
    pub token_b: String,
    /// Price of token B per token A in user display order (e.g. `"0.2"` means 0.2 B per A).
    pub price: String,
    /// Human decimal deposit amount (e.g. `"100"`).
    pub deposit: String,
    /// Which user-named token the deposit applies to (`token_a` or `token_b` label).
    pub deposit_token: String,
    /// Fee tier in bps; omitted → discover on chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee: Option<u32>,
    #[serde(default)]
    pub range: LpRangeInput,
}

/// JSON Brew file format — user-owned paths only (see `vaughan-agent/brews/brew.example.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LpDeployBrewFile {
    #[serde(default)]
    pub network_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
    pub token_a: String,
    pub token_b: String,
    pub price: String,
    pub deposit: String,
    pub deposit_token: String,
    #[serde(default)]
    pub fee: Option<u32>,
    #[serde(default)]
    pub range: LpRangeInput,
}

/// Format a float for on-chain human price strings (trim trailing zeros).
pub fn trim_float_string(v: f64) -> String {
    let s = format!("{v:.12}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Map user price (token B per token A) to pool price (token1 per token0).
pub fn user_price_to_pool_price(
    first_is_token0: bool,
    user_price: &str,
) -> Result<String, WalletError> {
    let raw = user_price.trim();
    if raw.is_empty() {
        return Ok(String::new());
    }
    let p: f64 = raw
        .parse()
        .map_err(|_| WalletError::InvalidAmount("invalid price — use decimal e.g. 0.2".into()))?;
    if p <= 0.0 {
        return Err(WalletError::InvalidAmount("price must be > 0".into()));
    }
    let pool = if first_is_token0 { p } else { 1.0 / p };
    Ok(trim_float_string(pool))
}

/// Map UI min/max (2nd per 1st) to ascending pool prices (token1 per token0).
pub fn user_price_range_to_pool_prices(
    first_is_token0: bool,
    user_min: &str,
    user_max: &str,
) -> Result<(String, String), WalletError> {
    let pool_from_min = user_price_to_pool_price(first_is_token0, user_min)?;
    let pool_from_max = user_price_to_pool_price(first_is_token0, user_max)?;
    if first_is_token0 {
        Ok((pool_from_min, pool_from_max))
    } else {
        Ok((pool_from_max, pool_from_min))
    }
}

/// Inverse of [`user_price_to_pool_price`] for display.
pub fn pool_price_to_user_price(first_is_token0: bool, pool_price: &str) -> String {
    let Ok(p) = pool_price.trim().parse::<f64>() else {
        return pool_price.trim().to_string();
    };
    if p <= 0.0 {
        return pool_price.trim().to_string();
    }
    let user = if first_is_token0 { p } else { 1.0 / p };
    trim_float_string(user)
}

/// Sort two tokens into on-chain `token0 < token1` order.
pub fn sort_lp_token_pair(
    token_a: Address,
    token_b: Address,
    dec_a: u8,
    dec_b: u8,
) -> SortedLpTokens {
    if token_a < token_b {
        SortedLpTokens {
            token0: token_a,
            token1: token_b,
            dec0: dec_a,
            dec1: dec_b,
            first_is_token0: true,
        }
    } else {
        SortedLpTokens {
            token0: token_b,
            token1: token_a,
            dec0: dec_b,
            dec1: dec_a,
            first_is_token0: false,
        }
    }
}

/// Resolve a token symbol or address for LP Brews on supported chains.
pub fn resolve_lp_brew_token(raw: &str, chain_id: u64) -> Result<Address, WalletError> {
    let s = raw.trim();
    if s.eq_ignore_ascii_case("native")
        || s.eq_ignore_ascii_case("pls")
        || s.eq_ignore_ascii_case("wpls")
    {
        return parse_addr(WPLS_943).ok_or_else(|| WalletError::InvalidTransaction("no WPLS".into()));
    }
    if s.eq_ignore_ascii_case("wzrd") {
        return parse_addr(WZRD_SMOKE_943)
            .ok_or_else(|| WalletError::InvalidTransaction("invalid WZRD".into()));
    }
    if chain_id == 943 {
        if s.eq_ignore_ascii_case("bob") {
            return Address::from_str("0x15de8ae884726f37ec90824f825d723ac93c8b77")
                .map_err(|_| WalletError::InvalidTransaction("BOB".into()));
        }
        if s.eq_ignore_ascii_case("jane") {
            return Address::from_str("0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59")
                .map_err(|_| WalletError::InvalidTransaction("JANE".into()));
        }
        if s.eq_ignore_ascii_case("jim") {
            return Address::from_str("0xc6ca0621683db4a03e31ad77e1d63eb3a03acbba")
                .map_err(|_| WalletError::InvalidTransaction("JIM".into()));
        }
    }
    if let Ok(addr) = Address::from_str(s) {
        return Ok(addr);
    }
    Err(WalletError::InvalidTransaction(format!(
        "unknown token {raw:?} on chain {chain_id} — use a checksummed 0x address"
    )))
}

/// Load a Brew preset JSON file from disk.
pub fn load_brew_file(path: &Path) -> Result<LpDeployBrewFile, WalletError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| WalletError::Other(format!("read brew file: {e}")))?;
    serde_json::from_str(&raw).map_err(|e| WalletError::Other(format!("parse brew JSON: {e}")))
}

/// Resolve fee tier: explicit or first on-chain pool for the pair.
pub async fn resolve_lp_brew_fee(
    rpc_url: &str,
    venue: DexVenue,
    chain_id: u64,
    token0: Address,
    token1: Address,
    fee: Option<u32>,
) -> Result<u32, WalletError> {
    if let Some(f) = fee {
        return Ok(f);
    }
    discover_v3_pool_fee_tier(rpc_url, venue, chain_id, token0, token1)
        .await?
        .ok_or_else(|| {
            WalletError::InvalidTransaction(
                "no pool found at any fee tier — set fee explicitly for new pools".into(),
            )
        })
}

/// Build [`V3LpDeployParams`] from human Brew inputs (async: fee discover + deposit preview).
pub async fn lp_human_inputs_to_deploy_params(
    inputs: &LpHumanInputs,
) -> Result<V3LpDeployParams, WalletError> {
    let venue = inputs
        .venue
        .as_deref()
        .and_then(parse_dex_venue_label)
        .or_else(|| default_lp_v3_venue(inputs.chain_id))
        .ok_or_else(|| {
            WalletError::Other(format!(
                "no default V3 LP venue on chain {}",
                inputs.chain_id
            ))
        })?;

    let token_a = resolve_lp_brew_token(&inputs.token_a, inputs.chain_id)?;
    let token_b = resolve_lp_brew_token(&inputs.token_b, inputs.chain_id)?;
    let dec_a = fetch_erc20_decimals(&inputs.rpc_url, token_a).await?;
    let dec_b = fetch_erc20_decimals(&inputs.rpc_url, token_b).await?;
    let pair = sort_lp_token_pair(token_a, token_b, dec_a, dec_b);

    let fee = resolve_lp_brew_fee(
        &inputs.rpc_url,
        venue,
        inputs.chain_id,
        pair.token0,
        pair.token1,
        inputs.fee,
    )
    .await?;

    let pool_initial =
        user_price_to_pool_price(pair.first_is_token0, &inputs.price)?;

    let (pool_min, pool_max) = match &inputs.range {
        LpRangeInput::Full => (String::new(), String::new()),
        LpRangeInput::MinMax { min, max } => {
            user_price_range_to_pool_prices(pair.first_is_token0, min, max)?
        }
    };

    let deposit_addr = resolve_lp_brew_token(&inputs.deposit_token, inputs.chain_id)?;
    let deposit_on_token0 = deposit_addr == pair.token0;

    let (amount0, amount1) = compute_deposit_amounts(
        inputs,
        &pair,
        fee,
        venue,
        pool_initial.as_str(),
        &pool_min,
        &pool_max,
        deposit_on_token0,
    )
    .await?;

    Ok(V3LpDeployParams {
        from: inputs.from.clone(),
        venue,
        chain_id: inputs.chain_id,
        rpc_url: inputs.rpc_url.clone(),
        token0: pair.token0,
        token1: pair.token1,
        fee,
        dec0: pair.dec0,
        dec1: pair.dec1,
        pool_initial_price: pool_initial,
        pool_min_price: pool_min,
        pool_max_price: pool_max,
        amount0,
        amount1,
        deposit_on_token0,
    })
}
async fn fetch_erc20_decimals(rpc_url: &str, token: Address) -> Result<u8, WalletError> {
    use alloy::providers::ProviderBuilder;
    use alloy::sol;
    sol! {
        #[sol(rpc)]
        contract Erc20Decimals {
            function decimals() external view returns (uint8);
        }
    }
    let url = rpc_url
        .parse()
        .map_err(|_| WalletError::NetworkError("invalid RPC URL".into()))?;
    let provider = ProviderBuilder::new().connect_http(url);
    let c = Erc20Decimals::new(token, provider);
    c.decimals()
        .call()
        .await
        .map_err(|e| WalletError::NetworkError(format!("decimals: {e}")))
}

#[allow(clippy::too_many_arguments)]
async fn compute_deposit_amounts(
    inputs: &LpHumanInputs,
    pair: &SortedLpTokens,
    fee: u32,
    venue: DexVenue,
    pool_initial: &str,
    pool_min: &str,
    pool_max: &str,
    deposit_on_token0: bool,
) -> Result<(String, String), WalletError> {
    let deposit_dec = if deposit_on_token0 {
        pair.dec0
    } else {
        pair.dec1
    };
    let deposit_wei = parse_native_amount(inputs.deposit.trim(), deposit_dec)?;
    let deposit_u256 = alloy::primitives::U256::from_str(&deposit_wei)
        .map_err(|_| WalletError::InvalidAmount("invalid deposit".into()))?;

    let lifecycle = v3_pool_lifecycle(
        &inputs.rpc_url,
        venue,
        inputs.chain_id,
        pair.token0,
        pair.token1,
        fee,
    )
    .await?;

    let (sqrt, tick) = if matches!(lifecycle, V3PoolLifecycle::Ready) {
        let (_, info) = crate::core::dex_lp::load_v3_lp_pool(
            &inputs.rpc_url,
            venue,
            inputs.chain_id,
            pair.token0,
            pair.token1,
            fee,
        )
        .await?;
        (
            v3_pool_sqrt_u160(info.sqrt_price_x96)?,
            info.tick,
        )
    } else {
        v3_sqrt_and_tick_for_preview(
            inputs.chain_id,
            pair.token0,
            pair.token1,
            pair.dec0,
            pair.dec1,
            fee,
            None,
            None,
            pool_initial,
        )?
    };

    let (a0, a1) = if deposit_on_token0 {
        v3_preview_mint_deposits_from_amount0(
            inputs.chain_id,
            pair.token0,
            pair.token1,
            pair.dec0,
            pair.dec1,
            fee,
            sqrt,
            tick,
            pool_min,
            pool_max,
            deposit_u256,
        )?
    } else {
        let (b1, b0) = v3_preview_mint_deposits_from_amount1(
            inputs.chain_id,
            pair.token0,
            pair.token1,
            pair.dec0,
            pair.dec1,
            fee,
            sqrt,
            tick,
            pool_min,
            pool_max,
            deposit_u256,
        )?;
        (b0, b1)
    };

    Ok((
        format_display_amount(&a0.to_string(), pair.dec0, 12),
        format_display_amount(&a1.to_string(), pair.dec1, 12),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_price_inverts_when_first_is_token1() {
        let pool = user_price_to_pool_price(false, "0.2").unwrap();
        assert!((pool.parse::<f64>().unwrap() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn user_price_passes_through_when_first_is_token0() {
        let pool = user_price_to_pool_price(true, "0.2").unwrap();
        assert_eq!(pool, "0.2");
    }

    #[test]
    fn range_swaps_when_inverted() {
        let (min, max) = user_price_range_to_pool_prices(false, "0.1", "0.5").unwrap();
        let min_f: f64 = min.parse().unwrap();
        let max_f: f64 = max.parse().unwrap();
        assert!(min_f < max_f);
    }

    #[test]
    fn bob_jane_user_price_maps_to_pool_order() {
        let bob = Address::from_str("0x15de8ae884726f37ec90824f825d723ac93c8b77").unwrap();
        let jane = Address::from_str("0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59").unwrap();
        let pair = sort_lp_token_pair(bob, jane, 18, 18);
        assert!(pair.first_is_token0, "BOB is token0 on 943");
        let pool = user_price_to_pool_price(pair.first_is_token0, "0.2").unwrap();
        assert!((pool.parse::<f64>().unwrap() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn jim_jane_inverts_user_price_when_jim_is_token_a() {
        let jane = Address::from_str("0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59").unwrap();
        let jim = Address::from_str("0xc6ca0621683db4a03e31ad77e1d63eb3a03acbba").unwrap();
        let pair = sort_lp_token_pair(jim, jane, 18, 18);
        assert!(!pair.first_is_token0, "JANE is token0 for JIM/JANE");
        let pool = user_price_to_pool_price(pair.first_is_token0, "10").unwrap();
        assert!((pool.parse::<f64>().unwrap() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn brew_json_roundtrip() {
        let json = r#"{
            "token_a": "0x15de8ae884726f37ec90824f825d723ac93c8b77",
            "token_b": "0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59",
            "price": "0.2",
            "deposit": "100",
            "deposit_token": "0x15de8ae884726f37ec90824f825d723ac93c8b77",
            "fee": 20000
        }"#;
        let brew: LpDeployBrewFile = serde_json::from_str(json).unwrap();
        assert_eq!(brew.fee, Some(20_000));
        assert_eq!(brew.price, "0.2");
    }
}
