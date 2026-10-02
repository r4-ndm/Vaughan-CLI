//! Wrapped-native (WPLS / WETH) `deposit` / `withdraw` decoding.

use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;

use super::labels::label_address;
use super::{HumanizeContext, Humanized, HumanizerModule};
use crate::core::proposal_verify::VerifyRow;
use crate::core::transaction::format_display_amount;

sol! {
    interface IWeth {
        function deposit() external payable;
        function withdraw(uint256 wad) external;
    }
}

pub struct Weth;

impl HumanizerModule for Weth {
    fn name(&self) -> &'static str {
        "weth"
    }

    fn try_humanize(
        &self,
        ctx: &HumanizeContext<'_>,
        to: Address,
        value: U256,
        calldata: &[u8],
    ) -> Option<Humanized> {
        let wrapped = label_address(ctx.chain_id, to);

        if IWeth::depositCall::abi_decode(calldata).is_ok() {
            let amt = format_display_amount(&value.to_string(), ctx.native_decimals, 8);
            return Some(Humanized {
                summary: format!("Wrap {amt} {} into {wrapped}", ctx.native_symbol),
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "Wrap native".into(),
                    },
                    VerifyRow {
                        label: "Amount".into(),
                        value: format!("{amt} {}", ctx.native_symbol),
                    },
                    VerifyRow {
                        label: "Into".into(),
                        value: wrapped,
                    },
                ],
                warnings: Vec::new(),
            });
        }

        if let Ok(c) = IWeth::withdrawCall::abi_decode(calldata) {
            let amt = format_display_amount(&c.wad.to_string(), ctx.native_decimals, 8);
            return Some(Humanized {
                summary: format!("Unwrap {amt} {wrapped} into {}", ctx.native_symbol),
                rows: vec![
                    VerifyRow {
                        label: "Action".into(),
                        value: "Unwrap native".into(),
                    },
                    VerifyRow {
                        label: "Amount".into(),
                        value: format!("{amt} {wrapped}"),
                    },
                ],
                warnings: Vec::new(),
            });
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn deposit_wraps() {
        let ctx = HumanizeContext {
            chain_id: 369,
            from: Some(address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
            native_symbol: "PLS",
            native_decimals: 18,
        };
        let wpls: Address = "0xA1077a294dDE1B09bB078844df40758a5D0f9a27"
            .parse()
            .unwrap();
        let data = IWeth::depositCall {}.abi_encode();
        let h = Weth
            .try_humanize(&ctx, wpls, U256::from(10u64.pow(18)), &data)
            .unwrap();
        assert!(h.summary.contains("Wrap"));
        assert!(h.summary.contains("WPLS"));
    }
}
