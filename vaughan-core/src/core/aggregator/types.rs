//! Shared aggregator quote request / response.

use alloy::primitives::{Address, Bytes, U256};

use crate::error::WalletError;

use super::catalog::AggVenue;

/// How native PLS is spelled for a given API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeSentinel {
    /// `0x000…000` (PulseSwap).
    ZeroAddress,
    /// String `PLS` (Piteas) — handled in the Piteas adapter, not as Address.
    PlsString,
    /// `0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE` (Switch).
    EeeeAddress,
}

/// Quote input shared across aggregator clients.
#[derive(Debug, Clone)]
pub struct AggQuoteRequest {
    pub token_in: Address,
    pub token_out: Address,
    pub token_in_is_native: bool,
    pub token_out_is_native: bool,
    pub amount_in: U256,
    /// Slippage percent (e.g. `0.5` = 0.5%).
    pub slippage_percent: f64,
    pub account: Option<Address>,
}

/// Calldata ready for the wallet’s approve → send path.
#[derive(Debug, Clone)]
pub struct AggExecTx {
    pub to: Address,
    pub data: Bytes,
    pub value: U256,
}

/// Normalized quote from any live aggregator.
#[derive(Debug, Clone)]
pub struct AggQuote {
    pub venue: AggVenue,
    pub amount_in: U256,
    pub amount_out: U256,
    pub gas_estimate: Option<u64>,
    pub tx: AggExecTx,
    /// ERC-20 spend allowance target (usually the router / `tx.to`).
    pub spender: Address,
}

/// One venue's result from a parallel compare pass.
#[derive(Debug)]
pub struct AggQuoteOutcome {
    pub venue: AggVenue,
    pub result: Result<AggQuote, WalletError>,
}
