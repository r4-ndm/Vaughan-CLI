//! EIP-2612 `permit` and Permit2-style `permit` calldata decoding.
//!
//! Permits are gasless approvals: a signed message that lets a spender move
//! tokens. They are a common drainer vector, so an unlimited value or a
//! far-future deadline always raises a warning.

use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;

use super::labels::{is_known, label_address};
use super::{HumanizeContext, Humanized, HumanizerModule, Warning};
use crate::core::proposal_verify::VerifyRow;

/// Deadlines more than this many seconds past "now" are flagged. We cannot
/// read the clock in a pure decoder, so we compare against a fixed far-future
/// threshold (year 2100) — anything beyond is effectively unlimited.
const FAR_FUTURE_UNIX: u64 = 4_102_444_800;

sol! {
    interface IPermit {
        // EIP-2612
        function permit(address owner, address spender, uint256 value, uint256 deadline, uint8 v, bytes32 r, bytes32 s) external;
    }

    // Permit2 AllowanceTransfer single-token permit payload.
    struct PermitSingle {
        address token;
        uint160 amount;
        uint48 expiration;
        uint48 nonce;
    }
    interface IPermit2 {
        function permit(address owner, PermitSingle permitSingle, bytes signature) external;
    }
}

pub struct Permit;

impl HumanizerModule for Permit {
    fn name(&self) -> &'static str {
        "permit"
    }

    fn try_humanize(
        &self,
        ctx: &HumanizeContext<'_>,
        to: Address,
        _value: U256,
        calldata: &[u8],
    ) -> Option<Humanized> {
        if let Ok(c) = IPermit::permitCall::abi_decode(calldata) {
            let token = label_address(ctx.chain_id, to);
            let spender_label = label_address(ctx.chain_id, c.spender);
            let mut warnings = Vec::new();
            if c.value == U256::MAX {
                warnings.push(Warning::UnlimitedApproval { spender: c.spender });
            }
            if !is_known(ctx.chain_id, c.spender) {
                warnings.push(Warning::UnknownSpender { address: c.spender });
            }
            let deadline = c.deadline.to::<u64>();
            if deadline > FAR_FUTURE_UNIX {
                warnings.push(Warning::FarFutureDeadline {
                    deadline_unix: deadline,
                });
            }
            return Some(Humanized {
                summary: format!("Permit {spender_label} to spend {token} (gasless approval)"),
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "Permit (EIP-2612)".into(),
                    },
                    VerifyRow {
                        label: "Token".into(),
                        value: token,
                    },
                    VerifyRow {
                        label: "Owner".into(),
                        value: format!("{:#x}", c.owner),
                    },
                    VerifyRow {
                        label: "Spender".into(),
                        value: format!("{:#x} ({spender_label})", c.spender),
                    },
                    VerifyRow {
                        label: "Value".into(),
                        value: if c.value == U256::MAX {
                            "UNLIMITED".into()
                        } else {
                            c.value.to_string()
                        },
                    },
                    VerifyRow {
                        label: "Deadline".into(),
                        value: deadline.to_string(),
                    },
                ],
                warnings,
            });
        }

        // Permit2 AllowanceTransfer — value lives inside the struct.
        if let Ok(c) = IPermit2::permitCall::abi_decode(calldata) {
            let token = label_address(ctx.chain_id, c.permitSingle.token);
            let expiration = c.permitSingle.expiration.to::<u64>();
            let mut warnings = Vec::new();
            if expiration > FAR_FUTURE_UNIX {
                warnings.push(Warning::FarFutureDeadline {
                    deadline_unix: expiration,
                });
            }
            return Some(Humanized {
                summary: format!("Permit2 allowance for {token}"),
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "Permit2 allowance".into(),
                    },
                    VerifyRow {
                        label: "Token".into(),
                        value: token,
                    },
                    VerifyRow {
                        label: "Owner".into(),
                        value: format!("{:#x}", c.owner),
                    },
                    VerifyRow {
                        label: "Amount (raw)".into(),
                        value: c.permitSingle.amount.to_string(),
                    },
                    VerifyRow {
                        label: "Expiration".into(),
                        value: expiration.to_string(),
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
    use alloy::primitives::{address, B256};

    fn ctx() -> HumanizeContext<'static> {
        HumanizeContext {
            chain_id: 369,
            from: Some(address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
            native_symbol: "PLS",
            native_decimals: 18,
        }
    }

    #[test]
    fn unlimited_permit_warns() {
        let spender = address!("0x00000000000000000000000000000000deadbeef");
        let data = IPermit::permitCall {
            owner: ctx().from.unwrap(),
            spender,
            value: U256::MAX,
            deadline: U256::from(1_900_000_000u64),
            v: 27,
            r: B256::ZERO,
            s: B256::ZERO,
        }
        .abi_encode();
        let h = Permit
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
            .any(|w| matches!(w, Warning::UnlimitedApproval { .. })));
        assert!(h
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::UnknownSpender { .. })));
    }

    #[test]
    fn far_future_deadline_warns() {
        let spender = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9");
        let data = IPermit::permitCall {
            owner: ctx().from.unwrap(),
            spender,
            value: U256::from(1u64),
            deadline: U256::from(u64::MAX),
            v: 27,
            r: B256::ZERO,
            s: B256::ZERO,
        }
        .abi_encode();
        let h = Permit
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
            .any(|w| matches!(w, Warning::FarFutureDeadline { .. })));
    }
}
