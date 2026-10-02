//! ERC-20 `transfer` / `approve` / `transferFrom` and ERC-721/1155
//! `setApprovalForAll` decoding.

use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;

use super::labels::{is_known, label_address};
use super::{HumanizeContext, Humanized, HumanizerModule, Warning};
use crate::core::proposal_verify::VerifyRow;

sol! {
    interface IErc20 {
        function transfer(address to, uint256 amount) external returns (bool);
        function approve(address spender, uint256 amount) external returns (bool);
        function transferFrom(address from, address to, uint256 amount) external returns (bool);
    }
    interface IApprovalForAll {
        function setApprovalForAll(address operator, bool approved) external;
    }
}

pub struct Erc20;

impl HumanizerModule for Erc20 {
    fn name(&self) -> &'static str {
        "erc20"
    }

    fn try_humanize(
        &self,
        ctx: &HumanizeContext<'_>,
        to: Address,
        _value: U256,
        calldata: &[u8],
    ) -> Option<Humanized> {
        let token = label_address(ctx.chain_id, to);

        if let Ok(c) = IErc20::transferCall::abi_decode(calldata) {
            return Some(Humanized {
                summary: format!("Send {token} to {}", label_address(ctx.chain_id, c.to)),
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "Token transfer".into(),
                    },
                    VerifyRow {
                        label: "Token".into(),
                        value: token,
                    },
                    VerifyRow {
                        label: "To".into(),
                        value: format!("{:#x}", c.to),
                    },
                    VerifyRow {
                        label: "Amount (raw)".into(),
                        value: c.amount.to_string(),
                    },
                ],
                warnings: Vec::new(),
            });
        }

        if let Ok(c) = IErc20::approveCall::abi_decode(calldata) {
            let spender_label = label_address(ctx.chain_id, c.spender);
            let mut warnings = Vec::new();
            if c.amount == U256::MAX {
                warnings.push(Warning::UnlimitedApproval { spender: c.spender });
            }
            if !is_known(ctx.chain_id, c.spender) {
                warnings.push(Warning::UnknownSpender { address: c.spender });
            }
            let amount_txt = if c.amount == U256::MAX {
                "UNLIMITED".to_string()
            } else if c.amount.is_zero() {
                "0 (revoke)".to_string()
            } else {
                c.amount.to_string()
            };
            return Some(Humanized {
                summary: if c.amount == U256::MAX {
                    format!("Approve UNLIMITED {token} spending by {spender_label}")
                } else if c.amount.is_zero() {
                    format!("Revoke {token} approval for {spender_label}")
                } else {
                    format!("Approve {token} spending by {spender_label}")
                },
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "Token approval".into(),
                    },
                    VerifyRow {
                        label: "Token".into(),
                        value: token,
                    },
                    VerifyRow {
                        label: "Spender".into(),
                        value: format!("{:#x} ({spender_label})", c.spender),
                    },
                    VerifyRow {
                        label: "Amount".into(),
                        value: amount_txt,
                    },
                ],
                warnings,
            });
        }

        if let Ok(c) = IErc20::transferFromCall::abi_decode(calldata) {
            return Some(Humanized {
                summary: format!(
                    "Move {token} from {} to {}",
                    label_address(ctx.chain_id, c.from),
                    label_address(ctx.chain_id, c.to)
                ),
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "transferFrom".into(),
                    },
                    VerifyRow {
                        label: "Token".into(),
                        value: token,
                    },
                    VerifyRow {
                        label: "From".into(),
                        value: format!("{:#x}", c.from),
                    },
                    VerifyRow {
                        label: "To".into(),
                        value: format!("{:#x}", c.to),
                    },
                    VerifyRow {
                        label: "Amount (raw)".into(),
                        value: c.amount.to_string(),
                    },
                ],
                warnings: Vec::new(),
            });
        }

        if let Ok(c) = IApprovalForAll::setApprovalForAllCall::abi_decode(calldata) {
            let mut warnings = Vec::new();
            if c.approved {
                warnings.push(Warning::ApprovalForAll {
                    operator: c.operator,
                });
            }
            return Some(Humanized {
                summary: if c.approved {
                    format!("Grant {:#x} control of ALL NFTs in {token}", c.operator)
                } else {
                    format!("Revoke {:#x} control of NFTs in {token}", c.operator)
                },
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "setApprovalForAll".into(),
                    },
                    VerifyRow {
                        label: "Collection".into(),
                        value: token,
                    },
                    VerifyRow {
                        label: "Operator".into(),
                        value: format!("{:#x}", c.operator),
                    },
                    VerifyRow {
                        label: "Approved".into(),
                        value: c.approved.to_string(),
                    },
                ],
                warnings,
            });
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn ctx() -> HumanizeContext<'static> {
        HumanizeContext {
            chain_id: 369,
            from: Some(address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
            native_symbol: "PLS",
            native_decimals: 18,
        }
    }

    fn encode_approve(spender: Address, amount: U256) -> Vec<u8> {
        IErc20::approveCall { spender, amount }.abi_encode()
    }

    #[test]
    fn unlimited_approve_warns() {
        let spender = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9"); // PulseX V2
        let data = encode_approve(spender, U256::MAX);
        let h = Erc20
            .try_humanize(
                &ctx(),
                address!("0xA1077a294dDE1B09bB078844df40758a5D0f9a27"),
                U256::ZERO,
                &data,
            )
            .unwrap();
        assert!(h.summary.contains("UNLIMITED"));
        assert!(h
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::UnlimitedApproval { .. })));
        // PulseX V2 is a known router, so no UnknownSpender warning.
        assert!(!h
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::UnknownSpender { .. })));
    }

    #[test]
    fn approve_unknown_spender_warns() {
        let spender = address!("0x00000000000000000000000000000000deadbeef");
        let data = encode_approve(spender, U256::from(1000u64));
        let h = Erc20
            .try_humanize(
                &ctx(),
                address!("0xA1077a294dDE1B09bB078844df40758a5D0f9a27"),
                U256::ZERO,
                &data,
            )
            .unwrap();
        assert!(h
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::UnknownSpender { .. })));
    }

    #[test]
    fn set_approval_for_all_true_is_critical() {
        let op = address!("0x1111111111111111111111111111111111111111");
        let data = IApprovalForAll::setApprovalForAllCall {
            operator: op,
            approved: true,
        }
        .abi_encode();
        let h = Erc20
            .try_humanize(
                &ctx(),
                address!("0x2222222222222222222222222222222222222222"),
                U256::ZERO,
                &data,
            )
            .unwrap();
        assert!(h.warnings.iter().any(|w| w.is_critical()));
    }
}
