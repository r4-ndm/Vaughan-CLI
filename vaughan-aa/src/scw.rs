//! The `scw_transaction` data model and its signing digest.
//!
//! `scw_transaction` is Ambire's name for the off-chain payload that authorizes
//! a batch of calls from a smart account. We reimplement only the *schema* — the
//! fields the on-chain `AmbireAccount.execute` verifies — and write the digest
//! logic fresh from the ABI spec. See `docs/ambire-aa.md`.

use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::sol_types::SolValue;

use crate::abi::Transaction;

/// An off-chain smart-account transaction: the payload the user approves and the
/// account key signs. Reimplemented from the on-chain schema, not translated
/// from Ambire's TypeScript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScwTransaction {
    /// The smart-account address (the on-chain `address(this)` in the digest).
    pub account: Address,
    /// The chain id the batch is bound to (`block.chainid`).
    pub chain_id: u64,
    /// The account nonce, used for replay protection.
    pub nonce: u64,
    /// The ordered batch of calls.
    pub txns: Vec<Transaction>,
}

impl ScwTransaction {
    /// The digest preimage: `abi.encode(account, chainId, nonce, txns)`.
    ///
    /// This is byte-for-byte what `AmbireAccount.execute` hashes, so our digest
    /// matches the on-chain verifier by construction.
    ///
    /// We assemble the flat `abi.encode` layout by hand. Solidity's
    /// `abi.encode(account, chainId, nonce, txns)` emits the three static words,
    /// then the offset to the dynamic `txns` array (`0x80`, four head words),
    /// then the array's tail. `Vec::<Transaction>::abi_encode()` returns the
    /// array as a *single* dynamic value — `[self-offset][length][elements]` —
    /// so we drop that leading self-offset word to get the tail.
    pub fn encode_for_digest(&self) -> Vec<u8> {
        let txns = self.txns.abi_encode();
        debug_assert_eq!(U256::from_be_slice(&txns[..32]), U256::from(0x20u64));
        let txns_tail = &txns[32..];

        let mut out = Vec::with_capacity(4 * 32 + txns_tail.len());
        out.extend_from_slice(&self.account.abi_encode());
        out.extend_from_slice(&U256::from(self.chain_id).abi_encode());
        out.extend_from_slice(&U256::from(self.nonce).abi_encode());
        out.extend_from_slice(&U256::from(0x80u64).abi_encode());
        out.extend_from_slice(txns_tail);
        out
    }

    /// `keccak256(abi.encode(account, chainId, nonce, txns))`.
    pub fn digest(&self) -> B256 {
        keccak256(self.encode_for_digest())
    }
}

/// Ambire's on-chain `SignatureMode` byte — the trailing byte of the 66-byte
/// signature. Only the single-key EOA modes are reimplemented here.
///
/// The variant names are ours; the *values* are the contract's enum ordinals
/// (`EIP712` = 0, `EthSign` = 1 — the `EIP712` label is a legacy name in Ambire:
/// mode 0 signs the raw digest, not an EIP-712 struct).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SignatureMode {
    /// Sign the raw digest (`ecrecover` over the digest directly). On-chain `EIP712` = 0.
    RawHash = 0,
    /// Sign EIP-191 `"\x19Ethereum Signed Message:\n32" ‖ digest`. On-chain `EthSign` = 1.
    EthSign = 1,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Bytes, U256};

    fn sample() -> ScwTransaction {
        ScwTransaction {
            account: Address::from([0x11u8; 20]),
            chain_id: 1,
            nonce: 0,
            txns: vec![Transaction {
                to: Address::from([0x22u8; 20]),
                value: U256::ZERO,
                data: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            }],
        }
    }

    #[test]
    fn digest_matches_hand_computed_abi_encoding() {
        // Hand-built `abi.encode(address, uint256, uint256, Transaction[])` for
        // account=0x11..11, chainId=1, nonce=0, and a single txn with
        // to=0x22..22, value=0, data=0xdeadbeef. This is an independent
        // reference for the encoder, not produced by alloy's SolValue impl.
        //
        // Per the ABI spec, `T[]` is `enc(len) enc((X[0], ..., X[k-1]))`: the
        // array is the length, then the elements encoded as a *tuple*. Because
        // `Transaction` is dynamic, the 1-element tuple contributes a `0x20`
        // head offset before the element's own `to/value/data` encoding.
        let expected = concat!(
            // head: account (address), chainId, nonce, offset to txns (0x80)
            "0000000000000000000000001111111111111111111111111111111111111111",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000080",
            // txns: length, then the 1-tuple head offset (0x20), then the element
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000000000020",
            "0000000000000000000000002222222222222222222222222222222222222222",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000060",
            // data bytes: length, then right-padded payload
            "0000000000000000000000000000000000000000000000000000000000000004",
            "deadbeef00000000000000000000000000000000000000000000000000000000",
        );
        let encoded = hex::decode(expected).unwrap();
        assert_eq!(sample().encode_for_digest(), encoded);
        assert_eq!(sample().digest(), keccak256(&encoded));
    }

    #[test]
    fn digest_changes_with_nonce() {
        let mut a = sample();
        let mut b = sample();
        b.nonce = 1;
        assert_ne!(a.digest(), b.digest());
        a.nonce = 1;
        assert_eq!(a.digest(), b.digest());
    }
}
