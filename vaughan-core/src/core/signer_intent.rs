//! Typed intents for isolated signing daemon (`vaughan signerd`).
//!
//! # Security Invariant: No Calldata in Intents
//! Raw calldata must never cross the IPC boundary between agent/caller and signer.
//! [`SignerIntent`] contains typed fields only — destination addresses, token addresses,
//! amounts, and router specifications. The signer daemon constructs transaction calldata
//! internally from verified, re-derived parameters.
//!
//! To enforce this invariant at compile time, `SignerIntent` has no `calldata` member on
//! any variant, and `#[serde(deny_unknown_fields)]` rejects any payload containing a
//! `calldata` key.

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// DEX swap protocol family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapFamily {
    #[serde(alias = "v2", alias = "V2")]
    V2,
    #[serde(alias = "v3", alias = "V3")]
    V3,
    #[serde(
        alias = "agg",
        alias = "Agg",
        alias = "aggregator",
        alias = "Aggregator"
    )]
    Agg,
}

impl SwapFamily {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V2 => "v2",
            Self::V3 => "v3",
            Self::Agg => "agg",
        }
    }
}

/// A single recipient and amount within a batched transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchTransferLeg {
    pub to: Address,
    pub amount: U256,
}

/// Typed signer intent submitted to `vaughan signerd`.
///
/// Contains NO raw calldata member. All byte payloads are assembled internally
/// by the signer daemon after enforcing policy limits and re-deriving quotes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SignerIntent {
    /// Native or ERC-20 transfer.
    Transfer {
        chain_id: u64,
        to: Address,
        #[serde(default)]
        token: Option<Address>,
        amount: U256,
    },
    /// DEX or aggregator swap with explicit slippage bounds.
    Swap {
        chain_id: u64,
        family: SwapFamily,
        router: Address,
        path: Vec<Address>,
        amount_in: U256,
        min_amount_out: U256,
        #[serde(default)]
        fee: Option<u32>,
    },
    /// ERC-20 token allowance approval.
    Approve {
        chain_id: u64,
        token: Address,
        spender: Address,
        amount: U256,
        #[serde(default)]
        expiry: Option<u64>,
    },
    /// Wrap native currency to wrapped token (e.g. PLS -> WPLS, ETH -> WETH).
    Wrap { chain_id: u64, amount: U256 },
    /// Unwrap wrapped token to native currency (e.g. WPLS -> PLS, WETH -> ETH).
    Unwrap { chain_id: u64, amount: U256 },
    /// EIP-7702 batched native transfers.
    BatchTransfer {
        chain_id: u64,
        legs: Vec<BatchTransferLeg>,
    },
    /// DCA order slice execution (scheduled for S2).
    DcaSlice {
        plan_id: String,
        slice_index: u64,
        chain_id: u64,
        pair: Address,
        amount_in: U256,
        corridor_min_out: U256,
    },
    /// Stealth note sweep (scheduled for S2).
    SweepStealth {
        chain_id: u64,
        stealth_address: Address,
    },
}

impl SignerIntent {
    /// Chain ID target of this intent.
    pub fn chain_id(&self) -> u64 {
        match self {
            Self::Transfer { chain_id, .. } => *chain_id,
            Self::Swap { chain_id, .. } => *chain_id,
            Self::Approve { chain_id, .. } => *chain_id,
            Self::Wrap { chain_id, .. } => *chain_id,
            Self::Unwrap { chain_id, .. } => *chain_id,
            Self::BatchTransfer { chain_id, .. } => *chain_id,
            Self::DcaSlice { chain_id, .. } => *chain_id,
            Self::SweepStealth { chain_id, .. } => *chain_id,
        }
    }

    /// Short string tag for logging / metrics.
    pub fn intent_type(&self) -> &'static str {
        match self {
            Self::Transfer { .. } => "transfer",
            Self::Swap { .. } => "swap",
            Self::Approve { .. } => "approve",
            Self::Wrap { .. } => "wrap",
            Self::Unwrap { .. } => "unwrap",
            Self::BatchTransfer { .. } => "batch_transfer",
            Self::DcaSlice { .. } => "dca_slice",
            Self::SweepStealth { .. } => "sweep_stealth",
        }
    }

    /// Deterministic idempotency key for spend ledger tracking and replay prevention.
    pub fn idempotency_key(&self) -> String {
        let serialized = serde_json::to_vec(self).unwrap_or_default();
        let digest = hex::encode(Sha256::digest(&serialized));
        format!(
            "{}:{}:{}",
            self.chain_id(),
            self.intent_type(),
            &digest[..16]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_transfer_intent_deserializes() {
        let json = r#"{
            "type": "transfer",
            "chain_id": 369,
            "to": "0x1111111111111111111111111111111111111111",
            "amount": "1000000000000000000"
        }"#;
        let intent: SignerIntent = serde_json::from_str(json).unwrap();
        match intent {
            SignerIntent::Transfer {
                chain_id,
                to,
                token,
                amount,
            } => {
                assert_eq!(chain_id, 369);
                assert_eq!(
                    to,
                    Address::from_str("0x1111111111111111111111111111111111111111").unwrap()
                );
                assert_eq!(token, None);
                assert_eq!(amount, U256::from(1_000_000_000_000_000_000u64));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_calldata_key_is_refused_on_intent() {
        // Attempting to pass a raw calldata payload must fail at serde deserialization
        let json = r#"{
            "type": "transfer",
            "chain_id": 369,
            "to": "0x1111111111111111111111111111111111111111",
            "amount": "1000",
            "calldata": "0xa9059cbb000000000000000000000000"
        }"#;
        let err = serde_json::from_str::<SignerIntent>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown field `calldata`"),
            "expected unknown field calldata error, got: {err}"
        );
    }

    #[test]
    fn test_unknown_field_is_refused() {
        let json = r#"{
            "type": "wrap",
            "chain_id": 369,
            "amount": "100",
            "unexpected_dial": 42
        }"#;
        let err = serde_json::from_str::<SignerIntent>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown field `unexpected_dial`"),
            "expected unknown field error, got: {err}"
        );
    }

    #[test]
    fn test_unknown_variant_is_refused() {
        let json = r#"{
            "type": "propose_contract_call",
            "chain_id": 369,
            "to": "0x1111111111111111111111111111111111111111"
        }"#;
        let err = serde_json::from_str::<SignerIntent>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown variant"),
            "expected unknown variant error, got: {err}"
        );
    }

    #[test]
    fn test_swap_intent_deserializes() {
        let json = r#"{
            "type": "swap",
            "chain_id": 369,
            "family": "v3",
            "router": "0x1111111111111111111111111111111111111111",
            "path": [
                "0x2222222222222222222222222222222222222222",
                "0x3333333333333333333333333333333333333333"
            ],
            "amount_in": "1000",
            "min_amount_out": "950",
            "fee": 3000
        }"#;
        let intent: SignerIntent = serde_json::from_str(json).unwrap();
        match intent {
            SignerIntent::Swap {
                family,
                fee,
                amount_in,
                min_amount_out,
                ..
            } => {
                assert_eq!(family, SwapFamily::V3);
                assert_eq!(fee, Some(3000));
                assert_eq!(amount_in, U256::from(1000));
                assert_eq!(min_amount_out, U256::from(950));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_batch_transfer_unknown_field_refused() {
        let json = r#"{
            "type": "batch_transfer",
            "chain_id": 369,
            "legs": [
                {
                    "to": "0x1111111111111111111111111111111111111111",
                    "amount": "100",
                    "calldata": "0xdeadbeef"
                }
            ]
        }"#;
        let err = serde_json::from_str::<SignerIntent>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown field `calldata`"),
            "expected unknown field calldata on batch leg, got: {err}"
        );
    }

    #[test]
    fn test_idempotency_key_deterministic() {
        let intent1 = SignerIntent::Transfer {
            chain_id: 369,
            to: Address::repeat_byte(0x11),
            token: None,
            amount: U256::from(500),
        };
        let intent2 = SignerIntent::Transfer {
            chain_id: 369,
            to: Address::repeat_byte(0x11),
            token: None,
            amount: U256::from(500),
        };
        let intent3 = SignerIntent::Transfer {
            chain_id: 369,
            to: Address::repeat_byte(0x11),
            token: None,
            amount: U256::from(600),
        };
        assert_eq!(intent1.idempotency_key(), intent2.idempotency_key());
        assert_ne!(intent1.idempotency_key(), intent3.idempotency_key());
    }
}
