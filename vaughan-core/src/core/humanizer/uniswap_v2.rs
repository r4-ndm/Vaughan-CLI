//! Uniswap V2-style router swaps (PulseX V1/V2, 9mm, and forks).
//!
//! Decodes the exact-in swap family and flags when proceeds go to an address
//! other than the signer. Amounts are shown raw (path token decimals are not
//! resolved without a chain read); the summary names the token contracts.

use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;

use super::labels::label_address;
use super::{HumanizeContext, Humanized, HumanizerModule, Warning};
use crate::core::proposal_verify::VerifyRow;

sol! {
    interface IV2Router {
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external returns (uint256[] amounts);
        function swapTokensForExactTokens(uint256 amountOut, uint256 amountInMax, address[] path, address to, uint256 deadline) external returns (uint256[] amounts);
        function swapExactETHForTokens(uint256 amountOutMin, address[] path, address to, uint256 deadline) external payable returns (uint256[] amounts);
        function swapExactTokensForETH(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external returns (uint256[] amounts);
        function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external;
        function swapExactETHForTokensSupportingFeeOnTransferTokens(uint256 amountOutMin, address[] path, address to, uint256 deadline) external payable;
        function swapExactTokensForETHSupportingFeeOnTransferTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external;
    }
}

pub struct UniswapV2;

/// Shared fields extracted from any V2 swap variant.
struct Swap {
    verb: &'static str,
    amount_in: Option<U256>,
    min_out: Option<U256>,
    path: Vec<Address>,
    to: Address,
}

impl HumanizerModule for UniswapV2 {
    fn name(&self) -> &'static str {
        "uniswap_v2"
    }

    fn try_humanize(
        &self,
        ctx: &HumanizeContext<'_>,
        _to: Address,
        value: U256,
        calldata: &[u8],
    ) -> Option<Humanized> {
        let swap = decode_swap(calldata)?;

        let in_label = swap
            .path
            .first()
            .map(|a| label_address(ctx.chain_id, *a))
            .unwrap_or_else(|| "?".into());
        let out_label = swap
            .path
            .last()
            .map(|a| label_address(ctx.chain_id, *a))
            .unwrap_or_else(|| "?".into());

        let mut warnings = Vec::new();
        if let Some(from) = ctx.from {
            if swap.to != from {
                warnings.push(Warning::RecipientNotSender { recipient: swap.to });
            }
        }

        let mut rows = vec![
            VerifyRow {
                label: "Action".into(),
                value: format!("Swap ({})", swap.verb),
            },
            VerifyRow {
                label: "From token".into(),
                value: in_label.clone(),
            },
            VerifyRow {
                label: "To token".into(),
                value: out_label.clone(),
            },
            VerifyRow {
                label: "Recipient".into(),
                value: format!("{:#x}", swap.to),
            },
        ];
        if let Some(a) = swap.amount_in {
            rows.push(VerifyRow {
                label: "Amount in (raw)".into(),
                value: a.to_string(),
            });
        } else if !value.is_zero() {
            rows.push(VerifyRow {
                label: "Amount in".into(),
                value: format!(
                    "{} {}",
                    crate::core::transaction::format_display_amount(
                        &value.to_string(),
                        ctx.native_decimals,
                        8
                    ),
                    ctx.native_symbol
                ),
            });
        }
        if let Some(m) = swap.min_out {
            rows.push(VerifyRow {
                label: "Min out (raw)".into(),
                value: m.to_string(),
            });
        }
        rows.push(VerifyRow {
            label: "Hops".into(),
            value: swap.path.len().saturating_sub(1).to_string(),
        });

        Some(Humanized {
            summary: format!("Swap {in_label} → {out_label}"),
            rows,
            warnings,
        })
    }
}

fn decode_swap(calldata: &[u8]) -> Option<Swap> {
    if let Ok(c) = IV2Router::swapExactTokensForTokensCall::abi_decode(calldata) {
        return Some(Swap {
            verb: "exact tokens for tokens",
            amount_in: Some(c.amountIn),
            min_out: Some(c.amountOutMin),
            path: c.path,
            to: c.to,
        });
    }
    if let Ok(c) = IV2Router::swapTokensForExactTokensCall::abi_decode(calldata) {
        return Some(Swap {
            verb: "tokens for exact tokens",
            amount_in: Some(c.amountInMax),
            min_out: Some(c.amountOut),
            path: c.path,
            to: c.to,
        });
    }
    if let Ok(c) = IV2Router::swapExactETHForTokensCall::abi_decode(calldata) {
        return Some(Swap {
            verb: "native for tokens",
            amount_in: None,
            min_out: Some(c.amountOutMin),
            path: c.path,
            to: c.to,
        });
    }
    if let Ok(c) = IV2Router::swapExactTokensForETHCall::abi_decode(calldata) {
        return Some(Swap {
            verb: "tokens for native",
            amount_in: Some(c.amountIn),
            min_out: Some(c.amountOutMin),
            path: c.path,
            to: c.to,
        });
    }
    if let Ok(c) =
        IV2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::abi_decode(calldata)
    {
        return Some(Swap {
            verb: "exact tokens for tokens (FoT)",
            amount_in: Some(c.amountIn),
            min_out: Some(c.amountOutMin),
            path: c.path,
            to: c.to,
        });
    }
    if let Ok(c) =
        IV2Router::swapExactETHForTokensSupportingFeeOnTransferTokensCall::abi_decode(calldata)
    {
        return Some(Swap {
            verb: "native for tokens (FoT)",
            amount_in: None,
            min_out: Some(c.amountOutMin),
            path: c.path,
            to: c.to,
        });
    }
    if let Ok(c) =
        IV2Router::swapExactTokensForETHSupportingFeeOnTransferTokensCall::abi_decode(calldata)
    {
        return Some(Swap {
            verb: "tokens for native (FoT)",
            amount_in: Some(c.amountIn),
            min_out: Some(c.amountOutMin),
            path: c.path,
            to: c.to,
        });
    }
    None
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

    #[test]
    fn decodes_exact_tokens_for_tokens() {
        let from = ctx().from.unwrap();
        let path = vec![
            address!("0xA1077a294dDE1B09bB078844df40758a5D0f9a27"), // WPLS
            address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39"), // pHEX
        ];
        let data = IV2Router::swapExactTokensForTokensCall {
            amountIn: U256::from(1000u64),
            amountOutMin: U256::from(900u64),
            path,
            to: from,
            deadline: U256::from(1_900_000_000u64),
        }
        .abi_encode();
        let h = UniswapV2
            .try_humanize(
                &ctx(),
                address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9"),
                U256::ZERO,
                &data,
            )
            .unwrap();
        assert!(h.summary.contains("WPLS"));
        assert!(h.summary.contains("pHEX"));
        assert!(h.warnings.is_empty());
    }

    #[test]
    fn warns_when_recipient_not_sender() {
        let path = vec![
            address!("0xA1077a294dDE1B09bB078844df40758a5D0f9a27"),
            address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39"),
        ];
        let data = IV2Router::swapExactTokensForTokensCall {
            amountIn: U256::from(1u64),
            amountOutMin: U256::from(1u64),
            path,
            to: address!("0x00000000000000000000000000000000deadbeef"),
            deadline: U256::from(1_900_000_000u64),
        }
        .abi_encode();
        let h = UniswapV2
            .try_humanize(
                &ctx(),
                address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9"),
                U256::ZERO,
                &data,
            )
            .unwrap();
        assert!(h
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::RecipientNotSender { .. })));
    }
}
