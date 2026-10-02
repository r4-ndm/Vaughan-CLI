//! Ground-truth MCP proposal review: decoded intent + safety hints.
//!
//! Builds [`VerifyRow`] tables for Advisor approval cards from typed proposal
//! fields and known calldata selectors — never trusts agent `explanation`.

use alloy::primitives::Address;

use crate::core::denylist::{self, DenyEntry};
use crate::core::humanizer::{humanize, HumanizeContext, Warning};
use crate::core::proposal::{ProposalType, TxProposal};
use crate::core::proposal_verify::VerifyRow;
use crate::core::transaction::format_display_amount;

/// Decoded review for an MCP proposal (table + safety hints).
#[derive(Debug, Clone, Default)]
pub struct ProposalReview {
    pub rows: Vec<VerifyRow>,
    pub safety_hints: Vec<String>,
}

/// Build a human verification table + safety hints from a proposal.
///
/// Convenience wrapper over [`review_mcp_proposal_with`] for callers that do
/// not know the signer or the user's deny-list (bundled list still applies).
pub fn review_mcp_proposal(
    proposal: &TxProposal,
    native_symbol: &str,
    native_decimals: u8,
) -> ProposalReview {
    review_mcp_proposal_with(proposal, native_symbol, native_decimals, None, &[])
}

/// Build a human verification table + safety hints from a proposal.
///
/// Typed proposal fields produce the first rows; a calldata humanizer pass
/// (`core::humanizer`) adds decoded rows for generic contract calls and typed
/// warnings for every kind. `from` enables recipient-not-sender checks;
/// `user_denylist` is the profile's own deny-list (merged with the bundled one).
///
/// LP Brew steps should still use [`crate::core::lp_deploy_step_verify_rows`];
/// this covers the general typed / calldata cases.
pub fn review_mcp_proposal_with(
    proposal: &TxProposal,
    native_symbol: &str,
    native_decimals: u8,
    from: Option<Address>,
    user_denylist: &[DenyEntry],
) -> ProposalReview {
    let mut review = ProposalReview::default();
    // Generic contract calls get their decoded rows from the humanizer; typed
    // proposals already describe themselves and only take its warnings.
    let mut humanized_rows = false;
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
            // Decoded rows for generic calls come from the humanizer pass below.
            humanized_rows = true;
        }
    }

    if !proposal.simulation_success {
        review
            .safety_hints
            .push("Agent reported simulation failure — broadcast may revert".into());
    }

    // Ground-truth pass, independent of the typed fields above: deny-list on
    // the target, then the calldata humanizer. Runs last so critical warnings
    // are always present, and dedupes against hints the typed arms already added.
    if denylist::is_denied(proposal.to, user_denylist) {
        review.safety_hints.push(format!(
            "TARGET {:#x} is DENIED — {} — do NOT sign",
            proposal.to,
            denylist::deny_reason(proposal.to, user_denylist)
        ));
    }
    let ctx = HumanizeContext {
        chain_id: proposal.chain_id,
        from,
        native_symbol,
        native_decimals,
    };
    let h = humanize(
        &ctx,
        proposal.to,
        proposal.value_wei,
        proposal.calldata.as_ref(),
    );
    if humanized_rows {
        review.rows.push(VerifyRow {
            label: "Decoded".into(),
            value: h.summary,
        });
        review.rows.extend(h.rows);
    }
    for w in h.warnings {
        // Plain native sends have nothing to warn about; skip the generic
        // "unrecognised calldata" for typed proposals whose calldata the
        // builder produced (they are reviewed by their typed arm instead).
        if matches!(w, Warning::UnrecognizedCalldata) && !humanized_rows {
            continue;
        }
        let msg = w.message();
        if !review.safety_hints.contains(&msg) {
            review.safety_hints.push(msg);
        }
    }

    review
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::hex_stake::{ehex_address, encode_stake_start, phex_address};
    use crate::core::proposal::ProposalType;
    use alloy::primitives::{address, Bytes, U256};
    use alloy::sol;
    use alloy::sol_types::SolCall;

    sol! {
        interface IReviewErc20 {
            function approve(address spender, uint256 amount) external returns (bool);
        }
    }

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
        let calldata = encode_stake_start(U256::from(100_000_000u64), 365).unwrap();
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

    #[test]
    fn user_denylist_flags_target_with_reason() {
        let bad = address!("0x00000000000000000000000000000000deadbeef");
        let p = TxProposal::new(
            "t",
            ProposalType::NativeTransfer {
                to: bad,
                amount_wei: U256::from(1u64),
            },
            bad,
            U256::from(1u64),
            Bytes::new(),
            21_000,
            true,
            "pay",
        );
        let clean = review_mcp_proposal(&p, "PLS", 18);
        assert!(!clean.safety_hints.iter().any(|h| h.contains("DENIED")));

        let entries = vec![DenyEntry {
            address: format!("{bad:#x}"),
            reason: "reported drainer".into(),
        }];
        let r = review_mcp_proposal_with(&p, "PLS", 18, None, &entries);
        assert!(r
            .safety_hints
            .iter()
            .any(|h| h.contains("DENIED") && h.contains("reported drainer")));
    }

    #[test]
    fn plain_native_transfer_has_no_calldata_warning() {
        let to = address!("0x1111111111111111111111111111111111111111");
        let p = TxProposal::new(
            "t",
            ProposalType::NativeTransfer {
                to,
                amount_wei: U256::from(1u64),
            },
            to,
            U256::from(1u64),
            Bytes::new(),
            21_000,
            true,
            "pay",
        );
        let r = review_mcp_proposal(&p, "PLS", 18);
        assert!(r.safety_hints.is_empty(), "{:?}", r.safety_hints);
    }
}
