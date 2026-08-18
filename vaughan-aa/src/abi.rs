//! Ambire smart-account ABI — interface facts only.
//!
//! These are the function signatures and struct shapes of Ambire's deployed,
//! verified `AmbireAccount` contract (AGPL-3.0), carried over strictly as
//! *interface facts* required for calldata interop. The contract's
//! implementation is never copied or reimplemented — see `docs/ambire-aa.md`.

alloy::sol! {
    /// A single call in the batch: the account performs `call(to, value, data)`.
    #[derive(Debug, PartialEq, Eq)]
    struct Transaction {
        address to;
        uint256 value;
        bytes data;
    }

    /// The `AmbireAccount` smart account. Only the entry points we use are
    /// declared; the contract is Ambire's deployed, verified Solidity.
    interface AmbireAccount {
        /// Execute a batch of calls authenticated by a 66-byte
        /// `r ‖ s ‖ v ‖ mode` signature.
        function execute(Transaction[] calldata txns, bytes calldata signature) external;
        /// The account nonce (replay protection).
        function nonce() external view returns (uint256);
    }
}
