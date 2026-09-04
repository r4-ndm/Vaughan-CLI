//! Human-readable verification rows for MCP approval cards.
//!
//! Decodes trusted on-chain calldata + persisted Brew job params into short
//! label/value pairs the TUI renders as a bordered table before raw hex.

use std::path::Path;

use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::sol_types::SolCall;

use wiz4rd_sdk::abi::{INonfungiblePositionManager, IPancakeV3Factory, IPancakeV3Pool};

use crate::core::dex_catalog::venue_position_manager;
use crate::core::dex_lp::default_full_range_ticks;
use crate::core::lp_deploy::{lp_deploy_job_load, LpDeployJob};
use crate::core::proposal::TxProposal;
use crate::core::transaction::format_display_amount;
use crate::core::V3LpDeployParams;
use crate::error::WalletError;

alloy::sol! {
    #[sol(rpc)]
    interface IERC20Approve {
        function approve(address spender, uint256 amount) external returns (bool);
    }
}

/// One row in the human verification table on the approve screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyRow {
    pub label: String,
    pub value: String,
}

/// Build verification rows for one LP Brew deploy step proposal.
pub fn lp_deploy_step_verify_rows(
    profile_dir: &Path,
    proposal: &TxProposal,
    job_id: &str,
    step_label: &str,
    token0_label: &str,
    token1_label: &str,
) -> Result<Vec<VerifyRow>, WalletError> {
    let job = lp_deploy_job_load(profile_dir, job_id)?;
    let params = V3LpDeployParams::try_from(&job.params)?;
    lp_deploy_step_verify_rows_with_job(
        proposal,
        step_label,
        &job,
        &params,
        token0_label,
        token1_label,
    )
}

fn lp_deploy_step_verify_rows_with_job(
    proposal: &TxProposal,
    step_label: &str,
    job: &LpDeployJob,
    params: &V3LpDeployParams,
    token0_label: &str,
    token1_label: &str,
) -> Result<Vec<VerifyRow>, WalletError> {
    let venue = params.venue.label();
    let fee = format_v3_fee_bps(params.fee);
    let pair = format!("{token0_label} / {token1_label} · {venue} · {fee} fee");

    let mut rows = vec![VerifyRow {
        label: "Brew job".into(),
        value: job.job_id.clone(),
    }];

    match step_label {
        "createPool" => {
            rows.push(VerifyRow {
                label: "Step".into(),
                value: "Create pool".into(),
            });
            rows.push(VerifyRow {
                label: "Pool".into(),
                value: pair,
            });
            if let Ok(call) = IPancakeV3Factory::createPoolCall::abi_decode(&proposal.calldata) {
                let on_fee = format_v3_fee_bps(call.fee.to::<u32>());
                rows.push(VerifyRow {
                    label: "Fee tier".into(),
                    value: on_fee,
                });
            }
        }
        "initialize" => {
            rows.push(VerifyRow {
                label: "Step".into(),
                value: "Initialize pool".into(),
            });
            rows.push(VerifyRow {
                label: "Pool".into(),
                value: pair,
            });
            if !params.pool_initial_price.trim().is_empty() {
                rows.push(VerifyRow {
                    label: "Start price".into(),
                    value: format!(
                        "{} {token1_label} per {token0_label}",
                        params.pool_initial_price.trim()
                    ),
                });
            }
            if let Ok(call) = IPancakeV3Pool::initializeCall::abi_decode(&proposal.calldata) {
                rows.push(VerifyRow {
                    label: "sqrtPriceX96".into(),
                    value: call.sqrtPriceX96.to_string(),
                });
            }
        }
        label if label.starts_with("approve") => {
            let token_name = if label.contains("token1") {
                token1_label
            } else {
                token0_label
            };
            rows.push(VerifyRow {
                label: "Step".into(),
                value: format!("Enable {token_name} for LP"),
            });
            rows.push(VerifyRow {
                label: "Pool".into(),
                value: pair,
            });
            rows.push(VerifyRow {
                label: "Token".into(),
                value: token_name.into(),
            });
            if let Ok(call) = IERC20Approve::approveCall::abi_decode(&proposal.calldata) {
                rows.push(VerifyRow {
                    label: "Allowance".into(),
                    value: format_allowance(&call.amount),
                });
                rows.push(VerifyRow {
                    label: "Spender".into(),
                    value: format!("{:#x}", call.spender),
                });
            }
        }
        "add liquidity" => {
            rows.push(VerifyRow {
                label: "Step".into(),
                value: "Add liquidity (mint)".into(),
            });
            rows.push(VerifyRow {
                label: "Pool".into(),
                value: pair,
            });
            if let Ok(call) = INonfungiblePositionManager::mintCall::abi_decode(&proposal.calldata)
            {
                let p = call.params;
                rows.push(VerifyRow {
                    label: "Range".into(),
                    value: format_tick_range(
                        p.tickLower.as_i32(),
                        p.tickUpper.as_i32(),
                        params.fee,
                    ),
                });
                rows.push(VerifyRow {
                    label: format!("Deposit {token0_label}"),
                    value: format_display_amount(&p.amount0Desired.to_string(), params.dec0, 12),
                });
                rows.push(VerifyRow {
                    label: format!("Deposit {token1_label}"),
                    value: format_display_amount(&p.amount1Desired.to_string(), params.dec1, 12),
                });
                rows.push(VerifyRow {
                    label: "Recipient".into(),
                    value: format!("{:#x}", p.recipient),
                });
            } else {
                rows.push(VerifyRow {
                    label: format!("Deposit {token0_label}"),
                    value: params.amount0.clone(),
                });
                rows.push(VerifyRow {
                    label: format!("Deposit {token1_label}"),
                    value: params.amount1.clone(),
                });
                rows.push(VerifyRow {
                    label: "Range".into(),
                    value: format_tick_range_from_params(params)?,
                });
            }
        }
        other => {
            rows.push(VerifyRow {
                label: "Step".into(),
                value: other.into(),
            });
            rows.push(VerifyRow {
                label: "Pool".into(),
                value: pair,
            });
        }
    }

    Ok(rows)
}

/// Rows for the post-mint success flash (TUI chrome table after Brew completes).
pub fn lp_deploy_mint_success_rows(
    profile_dir: &Path,
    job_id: &str,
    token0_label: &str,
    token1_label: &str,
    tx_hash: &str,
    position_token_id: Option<U256>,
) -> Result<Vec<VerifyRow>, WalletError> {
    let job = lp_deploy_job_load(profile_dir, job_id)?;
    let params = V3LpDeployParams::try_from(&job.params)?;
    let venue = params.venue.label();
    let fee = format_v3_fee_bps(params.fee);
    let pair = format!("{token0_label} / {token1_label} · {venue} · {fee} fee");

    let mut rows = vec![
        VerifyRow {
            label: "Brew job".into(),
            value: job.job_id.clone(),
        },
        VerifyRow {
            label: "Pool".into(),
            value: pair,
        },
        VerifyRow {
            label: "Range".into(),
            value: format_tick_range_from_params(&params)?,
        },
        VerifyRow {
            label: format!("Deposit {token0_label}"),
            value: params.amount0.clone(),
        },
        VerifyRow {
            label: format!("Deposit {token1_label}"),
            value: params.amount1.clone(),
        },
    ];
    if let Some(id) = position_token_id {
        rows.push(VerifyRow {
            label: "Position NFT".into(),
            value: format!("#{}", id),
        });
    }
    rows.push(VerifyRow {
        label: "Tx".into(),
        value: short_tx_hash(tx_hash),
    });
    Ok(rows)
}

/// Parse NPM position NFT id from mint receipt logs (ERC721 `Transfer` from zero).
pub fn npm_mint_token_id_from_logs(npm: Address, logs: &[Log]) -> Option<U256> {
    let topic0 = erc721_transfer_topic();
    for log in logs {
        if log.address() != npm {
            continue;
        }
        let topics = log.topics();
        if topics.first()? != &topic0 || topics.len() < 4 || topics[1] != B256::ZERO {
            continue;
        }
        return Some(U256::from_be_slice(topics[3].as_slice()));
    }
    None
}

/// Resolve NPM + scan receipt logs for a mint tx hash.
pub async fn npm_mint_token_id_for_tx(
    adapter: &crate::chains::evm::EvmAdapter,
    venue: crate::core::dex_catalog::DexVenue,
    chain_id: u64,
    tx_hash: &str,
) -> Result<Option<U256>, WalletError> {
    use alloy::primitives::B256;
    use std::str::FromStr;

    let npm = venue_position_manager(venue, chain_id)
        .ok_or_else(|| WalletError::InvalidTransaction("NPM not configured for venue".into()))?;
    let parsed = B256::from_str(tx_hash.trim())
        .map_err(|_| WalletError::InvalidTransaction("invalid mint tx hash".into()))?;
    let receipt = adapter
        .with_provider(|provider| async move {
            provider
                .get_transaction_receipt(parsed)
                .await
                .map_err(|e| WalletError::RpcError(e.to_string()))
        })
        .await?;
    Ok(receipt.and_then(|r| npm_mint_token_id_from_logs(npm, r.inner.logs())))
}

/// Short title for LP Brew approval cards.
pub fn lp_deploy_step_verify_title(step_label: &str) -> &'static str {
    match step_label {
        "createPool" => "LP Brew — Create pool",
        "initialize" => "LP Brew — Initialize pool",
        label if label.starts_with("approve") => "LP Brew — Enable token",
        "add liquidity" => "LP Brew — Add liquidity",
        _ => "LP Brew step",
    }
}

/// Truncate a tx hash for table cells: `0xf1fd…c25`.
pub fn short_tx_hash(hash: &str) -> String {
    let s = hash.trim();
    if s.len() <= 14 {
        return s.to_string();
    }
    format!("{}…{}", &s[..6], &s[s.len() - 4..])
}

/// Truncate an address for table cells: `0x33df…2770`.
pub fn short_address(addr: Address) -> String {
    let s = format!("{addr:#x}");
    if s.len() <= 14 {
        return s;
    }
    format!("{}…{}", &s[..6], &s[s.len() - 4..])
}

fn erc721_transfer_topic() -> B256 {
    keccak256(b"Transfer(address,address,uint256)")
}

fn format_v3_fee_bps(fee: u32) -> String {
    let pct = fee as f64 / 10_000.0;
    if (pct - pct.round()).abs() < 1e-9 {
        format!("{}%", pct as u32)
    } else {
        format!("{pct:.2}%")
    }
}

fn format_allowance(amount: &U256) -> String {
    if amount.is_zero() {
        "Reset to 0".into()
    } else if *amount == U256::MAX {
        "Unlimited (max)".into()
    } else {
        amount.to_string()
    }
}

fn format_tick_range(tick_lower: i32, tick_upper: i32, fee: u32) -> String {
    if let Ok((full_lo, full_hi)) = default_full_range_ticks(fee) {
        if tick_lower == full_lo && tick_upper == full_hi {
            return format!("Full ({tick_lower} → {tick_upper})");
        }
    }
    format!("{tick_lower} → {tick_upper}")
}

fn format_tick_range_from_params(params: &V3LpDeployParams) -> Result<String, WalletError> {
    let (lo, hi) =
        if params.pool_min_price.trim().is_empty() || params.pool_max_price.trim().is_empty() {
            default_full_range_ticks(params.fee)?
        } else {
            crate::core::dex_lp::v3_lp_mint_tick_range(params)?
        };
    Ok(format_tick_range(lo, hi, params.fee))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Bytes;
    use std::str::FromStr;

    use crate::core::dex_catalog::DexVenue;
    use crate::core::lp_deploy::lp_deploy_job_create;
    use crate::core::proposal::ProposalType;

    fn sample_job() -> LpDeployJob {
        let dir = tempfile::tempdir().unwrap();
        let params = V3LpDeployParams {
            from: "0x9274c57e08D9CdAbCA11d7b9c1Db04466789574f".into(),
            venue: DexVenue::Wiz4rd,
            chain_id: 943,
            rpc_url: "http://localhost".into(),
            token0: Address::from_str("0x33df366093ef8ac488e5be40e7ee2eeac2142770").unwrap(),
            token1: Address::from_str("0xfc413180d3624349d111fd98ee76bc08a25bc655").unwrap(),
            fee: 20000,
            dec0: 18,
            dec1: 18,
            pool_initial_price: "3.333333333333".into(),
            pool_min_price: String::new(),
            pool_max_price: String::new(),
            amount0: "90.363684870695".into(),
            amount1: "300".into(),
            deposit_on_token0: false,
        };
        lp_deploy_job_create(dir.path(), "lp_test", &params, "test brew").unwrap()
    }

    #[test]
    fn mint_calldata_decodes_to_deposit_rows() {
        use crate::core::dex_catalog::DexVenue;
        use crate::core::dex_lp::v3_lp_sdk_config;
        use wiz4rd_sdk::tx::liquidity::build_mint_tx;

        let job = sample_job();
        let params = V3LpDeployParams::try_from(&job.params).unwrap();
        let config = v3_lp_sdk_config(DexVenue::Wiz4rd, 943, "http://127.0.0.1:8545").unwrap();
        let amount0 = U256::from_str("90363684870695").unwrap();
        let amount1 = U256::from_str("300000000000000000000").unwrap();
        let req = build_mint_tx(
            &config,
            params.token0,
            params.token1,
            params.fee,
            -887200,
            887200,
            amount0,
            amount1,
            amount0 * U256::from(95u64) / U256::from(100u64),
            amount1 * U256::from(95u64) / U256::from(100u64),
            params.from.parse().unwrap(),
            1_800_000_000,
        )
        .unwrap();
        let calldata = req.input.input().expect("mint calldata").to_vec();
        let proposal = TxProposal::new(
            "lp-test-mint",
            ProposalType::LpDeployStep {
                job_id: "lp_test".into(),
                step_label: "add liquidity".into(),
            },
            Address::from_str("0xf1b1d004dd8bfc618f977f6acad127a60c566745").unwrap(),
            U256::ZERO,
            Bytes::from(calldata),
            868_587,
            true,
            "test",
        );
        let rows = lp_deploy_step_verify_rows_with_job(
            &proposal,
            "add liquidity",
            &job,
            &params,
            "T1",
            "T2",
        )
        .unwrap();
        let deposit_t2 = rows
            .iter()
            .find(|r| r.label == "Deposit T2")
            .expect("T2 deposit row");
        assert_eq!(deposit_t2.value, "300");
        let range = rows.iter().find(|r| r.label == "Range").expect("range row");
        assert!(range.value.contains("Full"));
        assert!(range.value.contains("-887200"));
    }

    #[test]
    fn fee_bps_formats_percent() {
        assert_eq!(format_v3_fee_bps(20000), "2%");
        assert_eq!(format_v3_fee_bps(500), "0.05%");
    }

    #[test]
    fn mint_success_rows_include_deposits_and_tx() {
        let dir = tempfile::tempdir().unwrap();
        let params = V3LpDeployParams {
            from: "0x9274c57e08D9CdAbCA11d7b9c1Db04466789574f".into(),
            venue: DexVenue::Wiz4rd,
            chain_id: 943,
            rpc_url: "http://localhost".into(),
            token0: Address::from_str("0x33df366093ef8ac488e5be40e7ee2eeac2142770").unwrap(),
            token1: Address::from_str("0xfc413180d3624349d111fd98ee76bc08a25bc655").unwrap(),
            fee: 20000,
            dec0: 18,
            dec1: 18,
            pool_initial_price: "3.333333333333".into(),
            pool_min_price: String::new(),
            pool_max_price: String::new(),
            amount0: "90.363684870695".into(),
            amount1: "300".into(),
            deposit_on_token0: false,
        };
        let job = lp_deploy_job_create(dir.path(), "lp_test", &params, "test brew").unwrap();
        let rows = lp_deploy_mint_success_rows(
            dir.path(),
            &job.job_id,
            "T1",
            "T2",
            "0xf1fd51a5171039bd1696a3d3d1ef38b67f56ab19b24d8a912281c5f599994c25",
            Some(U256::from(6u64)),
        )
        .unwrap();
        assert!(rows.iter().any(|r| r.label == "Deposit T1"));
        assert!(rows
            .iter()
            .any(|r| r.label == "Deposit T2" && r.value == "300"));
        assert!(rows
            .iter()
            .any(|r| r.label == "Position NFT" && r.value == "#6"));
        assert!(rows
            .iter()
            .any(|r| r.label == "Tx" && r.value.contains('…')));
    }
}
