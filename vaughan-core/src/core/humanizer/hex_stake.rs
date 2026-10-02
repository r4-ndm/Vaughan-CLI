//! HEX `stakeStart` / `stakeEnd` decoding, reusing the catalogued HEX
//! addresses and protocol day bounds from `core::hex_stake`.

use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;

use super::{HumanizeContext, Humanized, HumanizerModule, Warning};
use crate::core::hex_stake::{ehex_address, phex_address, MAX_STAKE_DAYS, MIN_STAKE_DAYS};
use crate::core::proposal_verify::VerifyRow;
use crate::core::transaction::format_display_amount;

sol! {
    interface IHex {
        function stakeStart(uint256 newStakedHearts, uint256 newStakedDays) external returns (uint40);
        function stakeEnd(uint256 stakeIndex, uint40 stakeId) external;
    }
}

pub struct HexStake;

impl HumanizerModule for HexStake {
    fn name(&self) -> &'static str {
        "hex_stake"
    }

    fn try_humanize(
        &self,
        _ctx: &HumanizeContext<'_>,
        to: Address,
        _value: U256,
        calldata: &[u8],
    ) -> Option<Humanized> {
        if let Ok(c) = IHex::stakeStartCall::abi_decode(calldata) {
            let days = c.newStakedDays.to::<u64>();
            let hearts = format_display_amount(&c.newStakedHearts.to_string(), 8, 8);
            let mut h = Humanized {
                summary: format!("Stake {hearts} HEX for {days} days"),
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "HEX stakeStart".into(),
                    },
                    VerifyRow {
                        label: "Hearts".into(),
                        value: hearts,
                    },
                    VerifyRow {
                        label: "Days".into(),
                        value: days.to_string(),
                    },
                ],
                warnings: Vec::new(),
            };
            if !(MIN_STAKE_DAYS..=MAX_STAKE_DAYS).contains(&days) {
                h.warnings.push(Warning::Advisory(format!(
                    "Stake days {days} outside protocol range {MIN_STAKE_DAYS}–{MAX_STAKE_DAYS}"
                )));
            }
            push_target_note(&mut h.warnings, to, "staking lives on pHEX, not eHEX");
            h.warnings.push(Warning::Advisory(
                "HEX stakes lock hearts for the full term; early endStake incurs a penalty".into(),
            ));
            return Some(h);
        }

        if let Ok(c) = IHex::stakeEndCall::abi_decode(calldata) {
            let mut h = Humanized {
                summary: "End a HEX stake (early end incurs a penalty)".into(),
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "HEX stakeEnd".into(),
                    },
                    VerifyRow {
                        label: "Stake index".into(),
                        value: c.stakeIndex.to_string(),
                    },
                    VerifyRow {
                        label: "Stake id".into(),
                        value: c.stakeId.to_string(),
                    },
                ],
                warnings: vec![Warning::Advisory(
                    "Ending before maturity applies an early-end penalty — confirm unlockedDay / days served"
                        .into(),
                )],
            };
            push_target_note(&mut h.warnings, to, "stakeEnd belongs on pHEX");
            return Some(h);
        }

        None
    }
}

/// Warn when the target is eHEX (bridged, not stakeable) or an unknown HEX address.
fn push_target_note(warnings: &mut Vec<Warning>, target: Address, ehex_note: &str) {
    if target == ehex_address() {
        warnings.push(Warning::Advisory(format!(
            "Target is eHEX (bridged) — {ehex_note}"
        )));
    } else if target != phex_address() {
        warnings.push(Warning::Advisory(
            "Target is not the catalogued pHEX address — verify carefully".into(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::hex_stake::{encode_stake_end, encode_stake_start};
    use alloy::primitives::address;

    fn ctx() -> HumanizeContext<'static> {
        HumanizeContext {
            chain_id: 369,
            from: Some(address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
            native_symbol: "PLS",
            native_decimals: 18,
        }
    }

    #[test]
    fn stake_start_summary() {
        let data = encode_stake_start(U256::from(100_000_000u64), 365).unwrap();
        let h = HexStake
            .try_humanize(&ctx(), phex_address(), U256::ZERO, &data)
            .unwrap();
        assert!(h.summary.contains("Stake"));
        assert!(h.summary.contains("365 days"));
    }

    #[test]
    fn stake_end_on_ehex_notes_target() {
        let data = encode_stake_end(0, 42).unwrap();
        let h = HexStake
            .try_humanize(&ctx(), ehex_address(), U256::ZERO, &data)
            .unwrap();
        assert!(h.warnings.iter().any(|w| w.message().contains("eHEX")));
        assert!(h.warnings.iter().all(|w| !w.is_critical()));
    }
}
