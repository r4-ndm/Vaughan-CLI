//! Small helpers for the EVM adapter.

use std::str::FromStr;

use alloy::primitives::Address;

use crate::error::WalletError;

/// Parse a hex address into an Alloy [`Address`].
pub fn parse_address(address: &str) -> Result<Address, WalletError> {
    Address::from_str(address.trim())
        .map_err(|e| WalletError::InvalidTransaction(format!("Invalid address {address}: {e}")))
}

/// Parse a decimal or `0x`-prefixed hex string into a [`alloy::primitives::U256`].
pub fn parse_u256(value: &str) -> Result<alloy::primitives::U256, WalletError> {
    let value = value.trim();
    if value.starts_with("0x") || value.starts_with("0X") {
        alloy::primitives::U256::from_str(value)
            .map_err(|_| WalletError::InvalidAmount(format!("Invalid hex amount: {value}")))
    } else {
        alloy::primitives::U256::from_str(value)
            .map_err(|_| WalletError::InvalidAmount(format!("Invalid amount: {value}")))
    }
}
