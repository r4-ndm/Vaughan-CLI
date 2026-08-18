//! Transaction orchestration: building native transfers, attaching fee
//! parameters, and delegating estimate/send to a [`ChainAdapter`].
//!
//! Phase 1 is EVM-only, so the builder produces an [`EvmTransaction`]; the
//! family-agnostic [`ChainAdapter`] contract is used for estimate and send so
//! future families slot in without changing callers.

use std::str::FromStr;

use alloy::primitives::utils::{format_units, parse_units};
use alloy::primitives::U256;

use crate::chains::{ChainAdapter, ChainTransaction, EvmTransaction, Fee, FeeDetails, TxHash};
use crate::error::WalletError;

/// Builds, estimates, and submits native transfers.
#[derive(Default)]
pub struct TransactionService;

impl TransactionService {
    pub fn new() -> Self {
        Self
    }

    /// Build an unsigned native-transfer request.
    ///
    /// `value` is in the chain's base unit (wei for EVM) as a decimal string.
    /// The amount is validated here (guardrail #5); the adapter validates the
    /// recipient address and fills nonce/gas at estimate/send time.
    pub fn build_native_transfer(
        &self,
        from: impl Into<String>,
        to: impl Into<String>,
        value: impl Into<String>,
        chain_id: u64,
    ) -> Result<ChainTransaction, WalletError> {
        let value = value.into();
        U256::from_str(&value)
            .map_err(|_| WalletError::InvalidAmount(format!("invalid amount: {value}")))?;
        Ok(ChainTransaction::Evm(EvmTransaction {
            from: from.into(),
            to: to.into(),
            value,
            data: None,
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id,
        }))
    }

    /// Build an unsigned contract-call request (any calldata).
    ///
    /// `value` is in the chain's base unit (wei for EVM) as a decimal string;
    /// pass `"0"` for a pure call. `data` is the hex calldata (`0x`-prefixed
    /// or not; empty for a plain value transfer).
    pub fn build_contract_call(
        &self,
        from: impl Into<String>,
        to: impl Into<String>,
        data: &str,
        value: impl Into<String>,
        chain_id: u64,
    ) -> Result<ChainTransaction, WalletError> {
        let value = value.into();
        U256::from_str(&value)
            .map_err(|_| WalletError::InvalidAmount(format!("invalid amount: {value}")))?;
        let data = data.trim();
        let data = data.strip_prefix("0x").unwrap_or(data).to_string();
        if !data.is_empty() && hex::decode(&data).is_err() {
            return Err(WalletError::InvalidTransaction(
                "data must be valid hex".to_string(),
            ));
        }
        Ok(ChainTransaction::Evm(EvmTransaction {
            from: from.into(),
            to: to.into(),
            value,
            data: Some(format!("0x{data}")),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id,
        }))
    }

    /// Copy `fee`'s gas parameters onto an EVM transaction.
    ///
    /// The UI shows the [`Fee`] to the user and, on approval, the confirmed
    /// fee's parameters are applied before broadcast.
    pub fn apply_fee(&self, tx: &mut ChainTransaction, fee: &Fee) -> Result<(), WalletError> {
        let ChainTransaction::Evm(evm_tx) = tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".to_string(),
            ));
        };
        let FeeDetails::Evm {
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
        } = &fee.details
        else {
            return Err(WalletError::InvalidTransaction(
                "expected EVM fee details".to_string(),
            ));
        };
        evm_tx.gas_limit = Some(*gas_limit);
        evm_tx.max_fee_per_gas = max_fee_per_gas.clone();
        evm_tx.max_priority_fee_per_gas = max_priority_fee_per_gas.clone();
        Ok(())
    }

    /// Estimate the fee for `tx` via the adapter.
    pub async fn estimate_fee(
        &self,
        adapter: &dyn ChainAdapter,
        tx: &ChainTransaction,
    ) -> Result<Fee, WalletError> {
        adapter.estimate_fee(tx).await
    }

    /// Sign and broadcast `tx` via the adapter.
    pub async fn send(
        &self,
        adapter: &dyn ChainAdapter,
        tx: ChainTransaction,
    ) -> Result<TxHash, WalletError> {
        adapter.send_transaction(tx).await
    }
}

/// Parse a human-readable native amount (e.g. `"0.01"`) into base units (wei)
/// as a decimal string, using the token's `decimals`.
pub fn parse_native_amount(value: &str, decimals: u8) -> Result<String, WalletError> {
    let units = parse_units(value, decimals)
        .map_err(|_| WalletError::InvalidAmount(format!("invalid amount: {value}")))?;
    if units.is_negative() {
        return Err(WalletError::InvalidAmount(format!(
            "amount must be positive: {value}"
        )));
    }
    let wei: U256 = units.into();
    Ok(wei.to_string())
}

/// Format a raw base-unit amount (wei) as a human-readable decimal string
/// using `decimals` (the inverse of [`parse_native_amount`]). Trailing zeros
/// are trimmed for display; falls back to the raw string when parsing fails so
/// display paths never panic on bad data.
pub fn format_base_units(value: &str, decimals: u8) -> String {
    match U256::from_str(value) {
        Ok(units) => format_units(units, decimals)
            .map(|s| trim_trailing_zeros(&s))
            .unwrap_or_else(|_| value.to_string()),
        Err(_) => value.to_string(),
    }
}

/// Trim trailing fractional zeros (and a dangling `.`) from a decimal string.
fn trim_trailing_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chains::BitcoinTransaction;

    #[test]
    fn parse_native_amount_converts_decimals() {
        assert_eq!(
            parse_native_amount("0.01", 18).unwrap(),
            "10000000000000000"
        );
        assert_eq!(parse_native_amount("1.5", 9).unwrap(), "1500000000");
        assert!(parse_native_amount("-1.0", 18).is_err());
        assert!(parse_native_amount("abc", 18).is_err());
    }

    #[test]
    fn format_base_units_roundtrips() {
        assert_eq!(format_base_units("10000000000000000", 18), "0.01");
        assert_eq!(format_base_units("1500000000", 9), "1.5");
        assert_eq!(format_base_units("0", 18), "0");
        assert_eq!(format_base_units("1000000000000000000", 18), "1");
        assert_eq!(format_base_units("not-a-number", 18), "not-a-number");
    }

    #[test]
    fn build_native_transfer_valid() {
        let svc = TransactionService::new();
        let tx = svc
            .build_native_transfer("0xabc", "0xdef", "1000", 369)
            .unwrap();
        match tx {
            ChainTransaction::Evm(e) => {
                assert_eq!(e.from, "0xabc");
                assert_eq!(e.to, "0xdef");
                assert_eq!(e.value, "1000");
                assert_eq!(e.chain_id, 369);
                assert!(e.nonce.is_none());
            }
            _ => panic!("expected EVM variant"),
        }
    }

    #[test]
    fn build_contract_call_keeps_data_and_value() {
        let svc = TransactionService::new();
        let tx = svc
            .build_contract_call("0xabc", "0xdef", "0x1234", "500", 369)
            .unwrap();
        match tx {
            ChainTransaction::Evm(e) => {
                assert_eq!(e.to, "0xdef");
                assert_eq!(e.value, "500");
                assert_eq!(e.data.as_deref(), Some("0x1234"));
                assert_eq!(e.chain_id, 369);
            }
            _ => panic!("expected EVM variant"),
        }
    }

    #[test]
    fn build_contract_call_rejects_bad_hex() {
        let svc = TransactionService::new();
        assert!(svc
            .build_contract_call("0xabc", "0xdef", "zzz", "0", 369)
            .is_err());
    }

    #[test]
    fn build_contract_call_accepts_bare_hex_and_empty() {
        let svc = TransactionService::new();
        let tx = svc
            .build_contract_call("0xabc", "0xdef", "1234", "0", 369)
            .unwrap();
        match tx {
            ChainTransaction::Evm(e) => {
                assert_eq!(e.data.as_deref(), Some("0x1234"));
            }
            _ => panic!("expected EVM variant"),
        }
    }

    #[test]
    fn build_rejects_bad_amount() {
        let svc = TransactionService::new();
        assert!(svc
            .build_native_transfer("0xabc", "0xdef", "not-a-number", 369)
            .is_err());
    }

    #[test]
    fn apply_fee_sets_gas_params() {
        let svc = TransactionService::new();
        let mut tx = svc
            .build_native_transfer("0xabc", "0xdef", "1000", 369)
            .unwrap();
        let fee = Fee {
            total: "0.0001 PLS".into(),
            currency: "PLS".into(),
            details: FeeDetails::Evm {
                gas_limit: 21_000,
                max_fee_per_gas: Some("2000000000".into()),
                max_priority_fee_per_gas: Some("1500000000".into()),
            },
        };
        svc.apply_fee(&mut tx, &fee).unwrap();
        match tx {
            ChainTransaction::Evm(e) => {
                assert_eq!(e.gas_limit, Some(21_000));
                assert_eq!(e.max_fee_per_gas.as_deref(), Some("2000000000"));
                assert_eq!(e.max_priority_fee_per_gas.as_deref(), Some("1500000000"));
            }
            _ => panic!("expected EVM variant"),
        }
    }

    #[test]
    fn apply_fee_rejects_non_evm_tx() {
        let svc = TransactionService::new();
        let mut tx = ChainTransaction::Bitcoin(BitcoinTransaction {
            from: "bc1q".into(),
            to: "bc1q".into(),
            amount_sats: "1000".into(),
            inputs: vec![],
            change_address: None,
            fee_rate_sat_per_vbyte: None,
        });
        let fee = Fee {
            total: "0".into(),
            currency: "BTC".into(),
            details: FeeDetails::Evm {
                gas_limit: 21_000,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
            },
        };
        assert!(svc.apply_fee(&mut tx, &fee).is_err());
    }
}
