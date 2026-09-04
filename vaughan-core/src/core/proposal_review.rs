//! Ground-truth MCP proposal review: decoded intent + safety hints.
//!
//! Builds [`VerifyRow`] tables for Advisor approval cards from typed proposal
//! fields and known calldata selectors — never trusts agent `explanation`.

use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;

use crate::core::hex_stake::{ehex_address, phex_address, MAX_STAKE_DAYS, MIN_STAKE_DAYS};
use crate::core::proposal::{ProposalType, TxProposal};
use crate::core::proposal_verify::VerifyRow;
use crate::core::transaction::format_display_amount;

sol! {
    interface IReviewErc20 {
        function transfer(address to, uint256 amount) external returns (bool);
        function approve(address spender, uint256 amount) external returns (bool);
    }
    interface IReviewWeth {
        function deposit() external payable;
        function withdraw(uint256 wad) external;
    }
    interface IReviewHex {
        function stakeStart(uint256 newStakedHearts, uint256 newStakedDays) external returns (uint40);
        function stakeEnd(uint256 stakeIndex, uint40 stakeId) external;
    }
}

/// Decoded review for an MCP proposal (table + safety hints).
#[derive(Debug, Clone, Default)]
pub struct ProposalReview {
    pub rows: Vec<VerifyRow>,
    pub safety_hints: Vec<String>,
}

/// Build a human verification table + safety hints from a proposal.
///
/// LP Brew steps should still use [`crate::core::lp_deploy_step_verify_rows`];
/// this covers the general typed / calldata cases.
pub fn review_mcp_proposal(proposal: &TxProposal, native_symbol: &str, native_decimals: u8) -> ProposalReview {
    let mut review = ProposalReview::default();
    match &proposal.proposal_type {
        ProposalType::NativeTransfer { to, amount_wei } => {
            review.rows.push(VerifyRow {
                label: "Action".into(),
                value: "Native transfer".into(),
            });
            review.rows.push(VerifyRow {
                label: "To".into(),
                value: format!("{to:#x}"),
            });
            review.rows.push(VerifyRow {
                label: "Amount".into(),
                value: format!(
                    "{} {native_symbol}",
                    format_display_amount(&amount_wei.to_string(), native_decimals, 8)
                ),
            });
        }
        ProposalType::Erc20Transfer {
            token,
            recipient,
            amount,
        } => {
            review.rows.push(VerifyRow {
                label: "Action".into(),
                value: "ERC-20 transfer".into(),
            });
            review.rows.push(VerifyRow {
                label: "Token".into(),
                value: format!("{token:#x}"),
            });
            review.rows.push(VerifyRow {
                label: "Recipient".into(),
                value: format!("{recipient:#x}"),
            });
            review.rows.push(VerifyRow {
                label: "Amount (raw)".into(),
                value: amount.to_string(),
            });
        }
        ProposalType::DexSwap {
            router,
            path,
            amount_in,
            min_amount_out,
        } => {
            review.rows.push(VerifyRow {
                label: "Action".into(),
                value: "DEX / Agg swap".into(),
            });
            review.rows.push(VerifyRow {
                label: "Router".into(),
                value: format!("{router:#x}"),
            });
            review.rows.push(VerifyRow {
                label: "Path".into(),
                value: path
                    .iter()
                    .map(|a| format!("{a:#x}"))
                    .collect::<Vec<_>>()
                    .join(" → "),
            });
            review.rows.push(VerifyRow {
                label: "Amount in".into(),
                value: amount_in.to_string(),
            });
            review.rows.push(VerifyRow {
                label: "Min out".into(),
                value: min_amount_out.to_string(),
            });
            if min_amount_out.is_zero() {
                review.safety_hints.push(
                    "min_amount_out is 0 — no slippage floor; fill can be sanded to dust".into(),
                );
            }
            if !proposal.value_wei.is_zero() {
                review.rows.push(VerifyRow {
                    label: "Native in".into(),
                    value: format!(
                        "{} {native_symbol}",
                        format_display_amount(&proposal.value_wei.to_string(), native_decimals, 8)
                    ),
                });
            }
        }
        ProposalType::Batch7702 {
            target_count,
            total_value,
        } => {
            review.rows.push(VerifyRow {
                label: "Action".into(),
                value: "EIP-7702 batch".into(),
            });
            review.rows.push(VerifyRow {
                label: "Calls".into(),
                value: target_count.to_string(),
            });
            review.rows.push(VerifyRow {
                label: "Total value".into(),
                value: format!(
                    "{} {native_symbol}",
                    format_display_amount(&total_value.to_string(), native_decimals, 8)
                ),
            });
            review.safety_hints.push(
                "Batch executes multiple calls atomically — verify every leg before signing".into(),
            );
        }
        ProposalType::TokenLaunch {
            name,
            symbol,
            supply_human,
        } => {
            review.rows.push(VerifyRow {
                label: "Action".into(),
                value: "Deploy fixed-supply ERC-20".into(),
            });
            review.rows.push(VerifyRow {
                label: "Name".into(),
                value: name.clone(),
            });
            review.rows.push(VerifyRow {
                label: "Symbol".into(),
                value: symbol.clone(),
            });
            review.rows.push(VerifyRow {
                label: "Supply".into(),
                value: format!("{supply_human} (18 decimals)"),
            });
        }
        ProposalType::LpDeployStep { job_id, step_label } => {
            review.rows.push(VerifyRow {
                label: "Action".into(),
                value: format!("LP Brew · {step_label}"),
            });
            review.rows.push(VerifyRow {
                label: "Job".into(),
                value: job_id.clone(),
            });
            // Prefer lp_deploy_step_verify_rows in the TUI for full rows.
        }
        ProposalType::ContractCall {
            target,
            function_name,
        } => {
            review.rows.push(VerifyRow {
                label: "Action".into(),
                value: function_name
                    .clone()
                    .unwrap_or_else(|| "Contract call".into()),
            });
            review.rows.push(VerifyRow {
                label: "Target".into(),
                value: format!("{target:#x}"),
            });
            enrich_from_calldata(proposal, *target, &mut review, native_symbol, native_decimals);
        }
    }

    if proposal.chain_id == 369 || proposal.chain_id == 1 {
        // no-op placeholder for future mainnet-specific hints
    }
    if !proposal.simulation_success {
        review
            .safety_hints
            .push("Agent reported simulation failure — broadcast may revert".into());
    }

    review
}

fn enrich_from_calldata(
    proposal: &TxProposal,
    target: Address,
    review: &mut ProposalReview,
    native_symbol: &str,
    native_decimals: u8,
) {
    let data = proposal.calldata.as_ref();
    if data.is_empty() {
        if !proposal.value_wei.is_zero() {
            review.rows.push(VerifyRow {
                label: "Value".into(),
                value: format!(
                    "{} {native_symbol}",
                    format_display_amount(&proposal.value_wei.to_string(), native_decimals, 8)
                ),
            });
        }
        return;
    }

    if let Ok(c) = IReviewErc20::transferCall::abi_decode(data) {
        review.rows.push(VerifyRow {
            label: "Decoded".into(),
            value: "transfer(to, amount)".into(),
        });
        review.rows.push(VerifyRow {
            label: "To".into(),
            value: format!("{:#x}", c.to),
        });
        review.rows.push(VerifyRow {
            label: "Amount (raw)".into(),
            value: c.amount.to_string(),
        });
        return;
    }
    if let Ok(c) = IReviewErc20::approveCall::abi_decode(data) {
        review.rows.push(VerifyRow {
            label: "Decoded".into(),
            value: "approve(spender, amount)".into(),
        });
        review.rows.push(VerifyRow {
            label: "Spender".into(),
            value: format!("{:#x}", c.spender),
        });
        review.rows.push(VerifyRow {
            label: "Amount (raw)".into(),
            value: if c.amount == U256::MAX {
                "UNLIMITED (max uint256)".into()
            } else {
                c.amount.to_string()
            },
        });
        if c.amount == U256::MAX {
            review.safety_hints.push(
                "Unlimited approve — spender can drain this token until revoked".into(),
            );
        } else if c.amount.is_zero() {
            review
                .safety_hints
                .push("Approve 0 = revoke allowance".into());
        }
        return;
    }
    if IReviewWeth::depositCall::abi_decode(data).is_ok() {
        review.rows.push(VerifyRow {
            label: "Decoded".into(),
            value: "WETH9 deposit()".into(),
        });
        return;
    }
    if let Ok(c) = IReviewWeth::withdrawCall::abi_decode(data) {
        review.rows.push(VerifyRow {
            label: "Decoded".into(),
            value: "WETH9 withdraw(wad)".into(),
        });
        review.rows.push(VerifyRow {
            label: "Wad".into(),
            value: c.wad.to_string(),
        });
        return;
    }
    if let Ok(c) = IReviewHex::stakeStartCall::abi_decode(data) {
        review.rows.push(VerifyRow {
            label: "Decoded".into(),
            value: "HEX stakeStart".into(),
        });
        review.rows.push(VerifyRow {
            label: "Hearts".into(),
            value: format!(
                "{} (8 decimals)",
                format_display_amount(&c.newStakedHearts.to_string(), 8, 8)
            ),
        });
        let days = c.newStakedDays.to::<u64>();
        review.rows.push(VerifyRow {
            label: "Days".into(),
            value: days.to_string(),
        });
        if !(MIN_STAKE_DAYS..=MAX_STAKE_DAYS).contains(&days) {
            review.safety_hints.push(format!(
                "Stake days {days} outside protocol range {MIN_STAKE_DAYS}–{MAX_STAKE_DAYS}"
            ));
        }
        if target == ehex_address() {
            review.safety_hints.push(
                "Target is eHEX (bridged) — staking lives on pHEX, not eHEX".into(),
            );
        } else if target != phex_address() {
            review
                .safety_hints
                .push("Target is not the catalogued pHEX address — verify carefully".into());
        }
        review.safety_hints.push(
            "HEX stakes lock hearts for the full term; early endStake incurs a penalty".into(),
        );
        return;
    }
    if let Ok(c) = IReviewHex::stakeEndCall::abi_decode(data) {
        review.rows.push(VerifyRow {
            label: "Decoded".into(),
            value: "HEX stakeEnd".into(),
        });
        review.rows.push(VerifyRow {
            label: "Index".into(),
            value: c.stakeIndex.to_string(),
        });
        review.rows.push(VerifyRow {
            label: "Stake id".into(),
            value: c.stakeId.to_string(),
        });
        review.safety_hints.push(
            "Ending before maturity applies an early-end penalty — confirm unlockedDay / days served"
                .into(),
        );
        if target == ehex_address() {
            review.safety_hints.push(
                "Target is eHEX — stakeEnd belongs on pHEX".into(),
            );
        } else if target != phex_address() {
            review
                .safety_hints
                .push("Target is not the catalogued pHEX address — verify carefully".into());
        }
        return;
    }

    review.rows.push(VerifyRow {
        label: "Decoded".into(),
        value: "unknown selector — inspect calldata".into(),
    });
    review.safety_hints.push(
        "Calldata selector not recognised — verify target and data carefully before signing".into(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, Bytes, U256};
    use crate::core::hex_stake::encode_stake_start;
    use crate::core::proposal::ProposalType;

    #[test]
    fn reviews_unlimited_approve() {
        let calldata = Bytes::from(
            IReviewErc20::approveCall {
                spender: address!("0x1111111111111111111111111111111111111111"),
                amount: U256::MAX,
            }
            .abi_encode(),
        );
        let p = TxProposal::new(
            "t",
            ProposalType::ContractCall {
                target: address!("0x2222222222222222222222222222222222222222"),
                function_name: Some("approve".into()),
            },
            address!("0x2222222222222222222222222222222222222222"),
            U256::ZERO,
            calldata,
            60_000,
            true,
            "agent says trust me",
        );
        let r = review_mcp_proposal(&p, "PLS", 18);
        assert!(r.safety_hints.iter().any(|h| h.contains("Unlimited")));
        assert!(r.rows.iter().any(|row| row.value.contains("UNLIMITED")));
    }

    #[test]
    fn reviews_hex_stake_start() {
        let calldata = encode_stake_start(U256::from(1_000_000_00u64), 365).unwrap();
        let p = TxProposal::new(
            "t",
            ProposalType::ContractCall {
                target: phex_address(),
                function_name: Some("stakeStart".into()),
            },
            phex_address(),
            U256::ZERO,
            calldata,
            300_000,
            true,
            "stake",
        );
        let r = review_mcp_proposal(&p, "PLS", 18);
        assert!(r.rows.iter().any(|row| row.value.contains("stakeStart")));
        assert!(r.safety_hints.iter().any(|h| h.contains("penalty")));
    }

    #[test]
    fn reviews_hex_stake_end_warns_non_phex_target() {
        use crate::core::hex_stake::encode_stake_end;
        let calldata = encode_stake_end(0, 42).unwrap();
        let weird = address!("0x1111111111111111111111111111111111111111");
        let p = TxProposal::new(
            "t",
            ProposalType::ContractCall {
                target: weird,
                function_name: Some("stakeEnd".into()),
            },
            weird,
            U256::ZERO,
            calldata,
            300_000,
            true,
            "end",
        );
        let r = review_mcp_proposal(&p, "PLS", 18);
        assert!(r.rows.iter().any(|row| row.value.contains("stakeEnd")));
        assert!(r
            .safety_hints
            .iter()
            .any(|h| h.contains("not the catalogued pHEX")));
    }

    #[test]
    fn reviews_hex_stake_end_warns_ehex_target() {
        use crate::core::hex_stake::encode_stake_end;
        let calldata = encode_stake_end(1, 7).unwrap();
        let p = TxProposal::new(
            "t",
            ProposalType::ContractCall {
                target: ehex_address(),
                function_name: Some("stakeEnd".into()),
            },
            ehex_address(),
            U256::ZERO,
            calldata,
            300_000,
            true,
            "end",
        );
        let r = review_mcp_proposal(&p, "PLS", 18);
        assert!(r.safety_hints.iter().any(|h| h.contains("eHEX")));
    }
}
