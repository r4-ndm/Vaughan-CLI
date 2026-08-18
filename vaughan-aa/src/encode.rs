//! Calldata encoding for the smart account.

use alloy::sol_types::SolCall;

use crate::abi::{AmbireAccount, Transaction};

/// Encode the calldata for `AmbireAccount.execute(txns, signature)`, i.e.
/// `selector ‖ abi.encode(txns, signature)`. This is what gets broadcast as the
/// `data` of the transaction that submits the batch.
pub fn encode_execute(txns: &[Transaction], signature: &[u8]) -> Vec<u8> {
    AmbireAccount::executeCall {
        txns: txns.to_vec(),
        signature: signature.to_vec().into(),
    }
    .abi_encode()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{keccak256, Address, Bytes, U256};

    fn sample_txns() -> Vec<Transaction> {
        vec![Transaction {
            to: Address::from([0x22u8; 20]),
            value: U256::from(7u64),
            data: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
        }]
    }

    #[test]
    fn calldata_selects_execute_and_roundtrips() {
        let calldata = encode_execute(&sample_txns(), &[0u8; 66]);

        // Selector must be bytes4(keccak256("execute((address,uint256,bytes)[],bytes)"))
        // computed independently of sol!.
        let expected_selector = &keccak256(b"execute((address,uint256,bytes)[],bytes)")[..4];
        assert_eq!(&calldata[..4], expected_selector);

        // The rest must decode back to the same args.
        let decoded = AmbireAccount::executeCall::abi_decode(&calldata).unwrap();
        assert_eq!(decoded.txns, sample_txns());
        assert_eq!(decoded.signature.as_ref(), &[0u8; 66][..]);
    }
}
