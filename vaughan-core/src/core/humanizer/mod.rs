//! Transaction humanizer: turn calldata into plain-English intent + warnings.
//!
//! Every signing surface (MCP proposal, provider `eth_sendTransaction`, TUI
//! send/swap confirm) should render the *same* ground-truth summary, derived
//! only from the transaction's `to` / `value` / `calldata` — never from agent
//! or dApp supplied text.
//!
//! Design rules (CLAUDE.md):
//! - **Modular.** Each protocol is a small [`HumanizerModule`] behind a trait;
//!   adding a protocol is one file plus one registration line.
//! - **Battle-tested decoding.** All ABI decoding goes through Alloy's
//!   `sol!`-generated `SolCall` types (the same encoder the rest of the wallet
//!   uses), never hand-rolled byte slicing.
//! - **No network, no secrets.** Pure functions over already-known data.

mod erc20;
mod hex_stake;
mod labels;
mod permit;
mod uniswap_v2;
mod weth;

use alloy::primitives::{Address, U256};

use crate::core::proposal_verify::VerifyRow;

pub use labels::label_address;

/// A typed, user-facing warning about a transaction.
///
/// Typed (not free text) so the UI can style by severity and tests can match
/// on the variant rather than a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// `approve(spender, type(uint256).max)` — spender can drain the token.
    UnlimitedApproval { spender: Address },
    /// The spender / target is not in any known catalog.
    UnknownSpender { address: Address },
    /// Swap proceeds go to an address other than the signer.
    RecipientNotSender { recipient: Address },
    /// Native value is sent to a contract with no recognised function.
    ValueToUnknownContract { target: Address },
    /// `setApprovalForAll(operator, true)` — operator can move every NFT.
    ApprovalForAll { operator: Address },
    /// A permit / typed-data deadline far in the future.
    FarFutureDeadline { deadline_unix: u64 },
    /// Target / spender / verifying contract is on the phishing deny-list.
    DeniedAddress { address: Address },
    /// Calldata selector not recognised by any module.
    UnrecognizedCalldata,
    /// Protocol-specific note that has no structured class of its own
    /// (e.g. "early HEX stakeEnd incurs a penalty"). Informational, never
    /// critical. Prefer adding a structured variant when a class recurs.
    Advisory(String),
}

impl Warning {
    /// Short human sentence for the approval card.
    pub fn message(&self) -> String {
        match self {
            Warning::UnlimitedApproval { spender } => {
                format!("Unlimited approval — {spender:#x} can drain this token until revoked")
            }
            Warning::UnknownSpender { address } => {
                format!("Spender {address:#x} is not a known catalogued contract")
            }
            Warning::RecipientNotSender { recipient } => {
                format!("Swap proceeds go to {recipient:#x}, not your wallet")
            }
            Warning::ValueToUnknownContract { target } => {
                format!("Sending native value to {target:#x} with unrecognised calldata")
            }
            Warning::ApprovalForAll { operator } => {
                format!("setApprovalForAll — {operator:#x} can move ALL of this NFT collection")
            }
            Warning::FarFutureDeadline { deadline_unix } => {
                format!("Permit deadline {deadline_unix} is far in the future")
            }
            Warning::DeniedAddress { address } => {
                format!("{address:#x} is on the phishing deny-list — do NOT sign")
            }
            Warning::UnrecognizedCalldata => {
                "Calldata selector not recognised — verify target and data carefully".into()
            }
            Warning::Advisory(note) => note.clone(),
        }
    }

    /// True for warnings severe enough to need an extra explicit confirm.
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            Warning::DeniedAddress { .. } | Warning::ApprovalForAll { .. }
        )
    }
}

/// The humanized reading of one transaction.
#[derive(Debug, Clone, Default)]
pub struct Humanized {
    /// One-line plain-English summary, e.g. "Approve USDC spending by PulseX V2".
    pub summary: String,
    /// Label/value rows for the approval table.
    pub rows: Vec<VerifyRow>,
    /// Typed warnings, most severe last.
    pub warnings: Vec<Warning>,
}

/// Context a module needs beyond raw calldata.
///
/// Borrows the native symbol from the active network config so any symbol
/// (including custom networks) is shown verbatim.
#[derive(Debug, Clone, Copy)]
pub struct HumanizeContext<'a> {
    pub chain_id: u64,
    /// The account that will sign, when known. Recipient-not-sender warnings
    /// are only emitted when this is `Some`.
    pub from: Option<Address>,
    /// Native coin symbol (e.g. `PLS`) and decimals for value display.
    pub native_symbol: &'a str,
    pub native_decimals: u8,
}

/// One protocol decoder. Implementations must be total: return `None` when the
/// calldata is not theirs, never panic.
pub trait HumanizerModule {
    /// Stable name for logging / tests.
    fn name(&self) -> &'static str;
    /// Try to humanize `calldata` sent to `to`. `None` = not my protocol.
    fn try_humanize(
        &self,
        ctx: &HumanizeContext<'_>,
        to: Address,
        value: U256,
        calldata: &[u8],
    ) -> Option<Humanized>;
}

/// All registered modules, in priority order (most specific first).
fn modules() -> Vec<Box<dyn HumanizerModule>> {
    vec![
        Box::new(permit::Permit),
        Box::new(hex_stake::HexStake),
        Box::new(uniswap_v2::UniswapV2),
        Box::new(weth::Weth),
        Box::new(erc20::Erc20),
    ]
}

/// Humanize one contract interaction. Always returns a value; unrecognised
/// calldata yields a generic summary plus an [`Warning::UnrecognizedCalldata`].
pub fn humanize(ctx: &HumanizeContext<'_>, to: Address, value: U256, calldata: &[u8]) -> Humanized {
    for module in modules() {
        if let Some(h) = module.try_humanize(ctx, to, value, calldata) {
            return h;
        }
    }

    // Fallback: no module recognised the calldata.
    let mut h = Humanized {
        summary: if calldata.is_empty() {
            format!(
                "Send {} {}",
                crate::core::transaction::format_display_amount(
                    &value.to_string(),
                    ctx.native_decimals,
                    8
                ),
                ctx.native_symbol
            )
        } else {
            format!("Contract call to {}", label_address(ctx.chain_id, to))
        },
        rows: Vec::new(),
        warnings: Vec::new(),
    };
    if !calldata.is_empty() {
        h.warnings.push(Warning::UnrecognizedCalldata);
        if !value.is_zero() {
            h.warnings
                .push(Warning::ValueToUnknownContract { target: to });
        }
    }
    h
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
    fn plain_native_send_has_no_warnings() {
        let h = humanize(
            &ctx(),
            address!("0x1111111111111111111111111111111111111111"),
            U256::from(10u64.pow(18)),
            &[],
        );
        assert!(h.summary.contains("PLS"));
        assert!(h.warnings.is_empty());
    }

    #[test]
    fn unknown_calldata_warns() {
        let h = humanize(
            &ctx(),
            address!("0x2222222222222222222222222222222222222222"),
            U256::ZERO,
            &[0xde, 0xad, 0xbe, 0xef, 1, 2, 3],
        );
        assert!(h.warnings.contains(&Warning::UnrecognizedCalldata));
    }
}
