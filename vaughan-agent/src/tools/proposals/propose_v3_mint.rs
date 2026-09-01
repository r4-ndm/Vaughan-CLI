//! Proposal tool: V3 mint (open concentrated LP) → TxProposal.

use alloy::primitives::U256;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use wiz4rd_math::fee_tiers::tick_spacing;
use wiz4rd_math::{nearest_usable_tick, MAX_V3_TICK, MIN_V3_TICK};
use wiz4rd_sdk::tx::liquidity::build_mint_tx;
use wiz4rd_sdk::tx::swap::apply_slippage;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::v3_lp::{
    load_lp_pool, proposal_network_id, resolve_lp_venue, venue_param_schema,
};
use crate::tools::wiz4rd_common::resolve_token;
use crate::tools::{Tool, ToolContext};
use vaughan_core::core::is_allowed_dex_router;
use vaughan_core::core::transaction::parse_native_amount;
use vaughan_core::core::{
    sort_lp_token_pair, v3_pool_sqrt_u160, v3_preview_mint_deposits_from_amount0_ticks,
    v3_preview_mint_deposits_from_amount1_ticks,
};

#[derive(Default)]
pub struct ProposeV3MintTool;

impl ProposeV3MintTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProposeV3MintTool {
    fn name(&self) -> &str {
        "propose_v3_mint"
    }

    fn description(&self) -> &str {
        "Draft a V3 mint (open LP NFT) for Vaughan approval. Prefer get_v3_pool first on 943; \
         on 369 use venue 9mm. Never signs. May need ERC-20 approve to the position manager separately."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_a": {
                    "type": "string",
                    "description": "First token (address, WPLS, WZRD on 943)"
                },
                "token_b": {
                    "type": "string",
                    "description": "Second token"
                },
                "amount_a": {
                    "type": "string",
                    "description": "Desired amount of token_a in raw wei units (or use amount_a_human)"
                },
                "amount_b": {
                    "type": "string",
                    "description": "Desired amount of token_b in raw wei units (or use amount_b_human)"
                },
                "amount_a_human": {
                    "type": "string",
                    "description": "Human decimal amount for token_a — requires amount_b_human too (or use deposit_amount_human)"
                },
                "amount_b_human": {
                    "type": "string",
                    "description": "Human decimal amount for token_b — requires amount_a_human too"
                },
                "deposit_token": {
                    "type": "string",
                    "description": "One-sided deposit: token symbol/address (with deposit_amount_human)"
                },
                "deposit_amount_human": {
                    "type": "string",
                    "description": "One-sided human deposit; previews coupled amount on the other side"
                },
                "fee": { "type": "integer", "default": 500 },
                "tick_lower": {
                    "type": "integer",
                    "description": "Optional lower tick (else ±range_spacings around current)"
                },
                "tick_upper": {
                    "type": "integer",
                    "description": "Optional upper tick"
                },
                "range_spacings": {
                    "type": "integer",
                    "description": "When ticks omitted: half-width in tick spacings (default 10)",
                    "default": 10
                },
                "slippage_bps": {
                    "type": "integer",
                    "description": "Floor amounts = desired × (1 − bps/10000); default 50",
                    "default": 50
                },
                "venue": venue_param_schema()["venue"],
                "explanation": { "type": "string" }
            },
            "required": ["token_a", "token_b", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let recipient = context.active_address.ok_or_else(|| {
            AgentError::InvalidToolCall(
                "No active wallet — unlock Vaughan TUI or pass session account".into(),
            )
        })?;
        let token_a_s = args
            .get("token_a")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_a".into()))?;
        let token_b_s = args
            .get("token_b")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing token_b".into()))?;
        let fee = args.get("fee").and_then(|v| v.as_u64()).unwrap_or(500) as u32;
        let slippage_bps = args
            .get("slippage_bps")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as u32;
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?;
        let venue = resolve_lp_venue(&args, context.chain_id)?;

        let (token_a, _) = resolve_token(token_a_s, context.chain_id)?;
        let (token_b, _) = resolve_token(token_b_s, context.chain_id)?;
        if token_a == token_b {
            return Err(AgentError::InvalidToolCall(
                "token_a and token_b must differ".into(),
            ));
        }

        let (cfg, pool) = load_lp_pool(context, venue, token_a, token_b, fee).await?;

        let (tick_lower, tick_upper) = resolve_mint_tick_range(&args, &pool, fee)?;

        let (amount0_desired, amount1_desired) = resolve_mint_amounts(
            &args,
            context,
            token_a,
            token_b,
            &pool,
            tick_lower,
            tick_upper,
        )
        .await?;

        if (amount0_desired.is_zero() || amount1_desired.is_zero())
            && pool.tick >= tick_lower
            && pool.tick < tick_upper
        {
            return Err(AgentError::InvalidToolCall(
                "mint requires both token amounts when the pool price is inside the tick range — \
                 use deposit_amount_human + deposit_token for a one-sided deposit, or pass both \
                 amount_a_human and amount_b_human"
                    .into(),
            ));
        }

        let amount0_min = apply_slippage(amount0_desired, slippage_bps);
        let amount1_min = apply_slippage(amount1_desired, slippage_bps);

        let deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() + 600)
            .unwrap_or(u64::MAX);

        let tx = build_mint_tx(
            &cfg,
            pool.token0,
            pool.token1,
            fee,
            tick_lower,
            tick_upper,
            amount0_desired,
            amount1_desired,
            amount0_min,
            amount1_min,
            recipient,
            deadline,
        )
        .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;

        let npm = match tx.to {
            Some(alloy::primitives::TxKind::Call(a)) => a,
            _ => cfg.position_manager.ok_or_else(|| {
                AgentError::InvalidToolCall("mint tx missing position_manager".into())
            })?,
        };
        if !is_allowed_dex_router(context.chain_id, npm) {
            return Err(AgentError::InvalidToolCall(format!(
                "position_manager {npm:#x} not allowlisted for chain {}",
                context.chain_id
            )));
        }
        let calldata = tx
            .input
            .into_input()
            .ok_or_else(|| AgentError::InvalidToolCall("mint tx missing calldata".into()))?;

        let gas = vaughan_core::core::lp_deploy_estimate_gas_limit(
            &context.rpc_url,
            context.chain_id,
            &vaughan_core::chains::EvmTransaction {
                from: format!("{recipient:#x}"),
                to: format!("{npm:#x}"),
                value: "0".into(),
                data: Some(format!("0x{}", hex::encode(calldata.as_ref()))),
                gas_limit: None,
                gas_price: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                nonce: None,
                chain_id: context.chain_id,
            },
        )
        .await
        .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;

        let proposal = attach_estimated_fee(
            TxProposal::new(
                format!("v3_mint_{}", rand_id()),
                ProposalType::ContractCall {
                    target: npm,
                    function_name: Some("mint".into()),
                },
                npm,
                U256::ZERO,
                calldata,
                gas,
                true,
                format!(
                    "{explanation} [{} mint fee {fee} ticks [{tick_lower},{tick_upper}] \
                     amt0={amount0_desired} amt1={amount1_desired}]",
                    venue.label()
                ),
            )
            .with_chain(context.chain_id, proposal_network_id(context)),
            context,
        )
        .await;

        Ok(serde_json::to_value(&proposal)?)
    }
}

async fn fetch_decimals(rpc_url: &str, token: alloy::primitives::Address) -> Result<u8, AgentError> {
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
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;
    let provider = ProviderBuilder::new().connect_http(url);
    Erc20Decimals::new(token, provider)
        .decimals()
        .call()
        .await
        .map_err(|e| AgentError::ProviderError(format!("decimals: {e}")))
}

fn resolve_mint_tick_range(
    args: &Value,
    pool: &wiz4rd_sdk::pool::PoolInfo,
    fee: u32,
) -> Result<(i32, i32), AgentError> {
    let spacing = tick_spacing(fee)
        .ok_or_else(|| AgentError::InvalidToolCall(format!("unsupported fee tier {fee}")))?;
    match (
        args.get("tick_lower").and_then(|v| v.as_i64()),
        args.get("tick_upper").and_then(|v| v.as_i64()),
    ) {
        (Some(lo), Some(hi)) => {
            let raw_lo = lo as i32;
            let raw_hi = hi as i32;
            if raw_lo < MIN_V3_TICK || raw_hi > MAX_V3_TICK {
                return Err(AgentError::InvalidToolCall(format!(
                    "tick_lower/tick_upper must be within [{MIN_V3_TICK}, {MAX_V3_TICK}] \
                     (got {raw_lo} and {raw_hi})"
                )));
            }
            let lo = nearest_usable_tick(raw_lo, spacing);
            let hi = nearest_usable_tick(raw_hi, spacing);
            if lo >= hi {
                return Err(AgentError::InvalidToolCall(
                    "tick_lower must be < tick_upper".into(),
                ));
            }
            if lo != raw_lo || hi != raw_hi {
                return Err(AgentError::InvalidToolCall(format!(
                    "tick_lower/tick_upper must align to fee spacing ({spacing}): \
                     use {lo} and {hi} (got {raw_lo} and {raw_hi})"
                )));
            }
            Ok((lo, hi))
        }
        (None, None) => {
            let half = args
                .get("range_spacings")
                .and_then(|v| v.as_i64())
                .unwrap_or(10) as i32;
            let lo = nearest_usable_tick(pool.tick - half * spacing, spacing);
            let hi = nearest_usable_tick(pool.tick + half * spacing, spacing);
            if lo >= hi {
                return Err(AgentError::InvalidToolCall(
                    "auto tick range collapsed — widen range_spacings".into(),
                ));
            }
            Ok((lo, hi))
        }
        _ => Err(AgentError::InvalidToolCall(
            "pass both tick_lower and tick_upper, or neither".into(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn resolve_mint_amounts(
    args: &Value,
    context: &ToolContext,
    token_a: alloy::primitives::Address,
    token_b: alloy::primitives::Address,
    pool: &wiz4rd_sdk::pool::PoolInfo,
    tick_lower: i32,
    tick_upper: i32,
) -> Result<(U256, U256), AgentError> {
    let deposit_human = args
        .get("deposit_amount_human")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(deposit) = deposit_human {
        let deposit_token = args
            .get("deposit_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::InvalidToolCall("deposit_token required with deposit_amount_human".into())
            })?;
        let (dep_addr, _) = resolve_token(deposit_token, context.chain_id)?;
        let dec_a = fetch_decimals(&context.rpc_url, token_a).await?;
        let dec_b = fetch_decimals(&context.rpc_url, token_b).await?;
        let pair = sort_lp_token_pair(token_a, token_b, dec_a, dec_b);
        let deposit_dec = if dep_addr == pair.token0 {
            pair.dec0
        } else if dep_addr == pair.token1 {
            pair.dec1
        } else {
            return Err(AgentError::InvalidToolCall(
                "deposit_token must be token_a or token_b".into(),
            ));
        };
        let deposit_wei = U256::from_str(&parse_native_amount(deposit, deposit_dec).map_err(
            |e| AgentError::InvalidToolCall(e.user_message()),
        )?)
        .map_err(|e| AgentError::InvalidToolCall(format!("deposit wei: {e}")))?;
        let sqrt = v3_pool_sqrt_u160(pool.sqrt_price_x96)
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        let (a0, a1) = if dep_addr == pair.token0 {
            v3_preview_mint_deposits_from_amount0_ticks(
                sqrt,
                pool.tick,
                tick_lower,
                tick_upper,
                deposit_wei,
            )
            .map_err(|e| {
                AgentError::InvalidToolCall(format!(
                    "deposit preview failed (check range and amount): {e}"
                ))
            })?
        } else {
            v3_preview_mint_deposits_from_amount1_ticks(
                sqrt,
                pool.tick,
                tick_lower,
                tick_upper,
                deposit_wei,
            )
            .map_err(|e| {
                AgentError::InvalidToolCall(format!(
                    "deposit preview failed (check range and amount): {e}"
                ))
            })?
        };
        if pool.tick < tick_lower && dep_addr != pair.token0 {
            return Err(AgentError::InvalidToolCall(
                "pool price is below your tick range — one-sided deposit must be on token0".into(),
            ));
        }
        if pool.tick >= tick_upper && dep_addr != pair.token1 {
            return Err(AgentError::InvalidToolCall(
                "pool price is above your tick range — one-sided deposit must be on token1".into(),
            ));
        }
        return Ok((a0, a1));
    }

    let human_a = args.get("amount_a_human").and_then(|v| v.as_str());
    let human_b = args.get("amount_b_human").and_then(|v| v.as_str());
    if human_a.is_some() || human_b.is_some() {
        if human_a.is_none() || human_b.is_none() {
            return Err(AgentError::InvalidToolCall(
                "pass both amount_a_human and amount_b_human, or use deposit_amount_human + \
                 deposit_token for a one-sided deposit"
                    .into(),
            ));
        }
        let dec_a = fetch_decimals(&context.rpc_url, token_a).await?;
        let dec_b = fetch_decimals(&context.rpc_url, token_b).await?;
        let amount_a = U256::from_str(&parse_native_amount(human_a.unwrap(), dec_a).map_err(
            |e| AgentError::InvalidToolCall(e.user_message()),
        )?)
        .map_err(|e| AgentError::InvalidToolCall(format!("amount_a_human: {e}")))?;
        let amount_b = U256::from_str(&parse_native_amount(human_b.unwrap(), dec_b).map_err(
            |e| AgentError::InvalidToolCall(e.user_message()),
        )?)
        .map_err(|e| AgentError::InvalidToolCall(format!("amount_b_human: {e}")))?;
        return map_amounts_to_pool_order(token_a, token_b, pool, amount_a, amount_b);
    }

    let amount_a = U256::from_str(
        args.get("amount_a")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AgentError::InvalidToolCall(
                    "Missing amount_a — or use amount_a_human / deposit_amount_human".into(),
                )
            })?,
    )
    .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount_a: {e}")))?;
    let amount_b = U256::from_str(
        args.get("amount_b")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing amount_b".into()))?,
    )
    .map_err(|e| AgentError::InvalidToolCall(format!("Invalid amount_b: {e}")))?;
    map_amounts_to_pool_order(token_a, token_b, pool, amount_a, amount_b)
}

fn map_amounts_to_pool_order(
    token_a: alloy::primitives::Address,
    token_b: alloy::primitives::Address,
    pool: &wiz4rd_sdk::pool::PoolInfo,
    amount_a: U256,
    amount_b: U256,
) -> Result<(U256, U256), AgentError> {
    if token_a == pool.token0 && token_b == pool.token1 {
        Ok((amount_a, amount_b))
    } else if token_a == pool.token1 && token_b == pool.token0 {
        Ok((amount_b, amount_a))
    } else {
        Err(AgentError::InvalidToolCall(
            "token_a/token_b do not match pool tokens".into(),
        ))
    }
}

#[cfg(test)]
mod tick_range_tests {
    use super::*;
    use alloy::primitives::{Address, U256};
    use wiz4rd_sdk::pool::PoolInfo;
    use wiz4rd_sdk::pool_address::PoolKey;

    fn mock_pool(tick: i32) -> PoolInfo {
        PoolInfo {
            pool_key: PoolKey {
                token0: Address::ZERO,
                token1: Address::from([1u8; 20]),
                fee: 500,
            },
            pool: Address::from([2u8; 20]),
            token0: Address::ZERO,
            token1: Address::from([1u8; 20]),
            fee: 500,
            sqrt_price_x96: U256::from(1u64),
            tick,
            fee_protocol: 0,
            liquidity: 0,
        }
    }

    #[test]
    fn rejects_unaligned_custom_ticks() {
        let pool = mock_pool(0);
        let args = json!({ "tick_lower": 15, "tick_upper": 25 });
        let err = resolve_mint_tick_range(&args, &pool, 500)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("align to fee spacing"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn rejects_ticks_above_max_bound() {
        let pool = mock_pool(0);
        let args = json!({ "tick_lower": 887_260, "tick_upper": 887_273 });
        let err = resolve_mint_tick_range(&args, &pool, 500)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("887272") || err.contains("within"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn accepts_aligned_ticks_within_bounds() {
        let pool = mock_pool(100);
        let args = json!({ "tick_lower": -200, "tick_upper": 200 });
        let (lo, hi) = resolve_mint_tick_range(&args, &pool, 500).unwrap();
        assert_eq!(lo, -200);
        assert_eq!(hi, 200);
    }
}
