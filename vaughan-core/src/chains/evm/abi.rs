//! Alloy `sol!` ABI bindings for the token/asset features.
//!
//! Provenance (per the optimization-source policy in `docs/optimizations.md`):
//! - [`IERC20Metadata`] — EIP-20 (https://eips.ethereum.org/EIPS/eip-20) plus
//!   the optional metadata accessors (`symbol`/`name`/`decimals`) standardized
//!   by OpenZeppelin's ERC20Metadata. `balanceOf` is the only strictly
//!   required member; the metadata calls are best-effort with fallbacks (some
//!   tokens omit them).
//! - [`IMulticall3`] — Multicall3 by mds1 (https://github.com/mds1/multicall),
//!   the canonical batched-call contract deployed at `0xcA11bde05977b3631167028862bE2a173976CA11`
//!   on essentially every EVM chain (verified present on PulseChain mainnet —
//!   see `docs/optimizations.md`). `tryAggregate` with `requireSuccess=false`
//!   returns per-call success + return data, so one non-conforming token
//!   cannot fail the whole balance batch.

use alloy::sol;

sol! {
    /// EIP-20 + optional metadata surface used for token balances.
    ///
    /// `Transfer` is the event auto asset detection scans (EIP-20 §1, the
    /// only event the standard requires) — see `EvmAdapter::discover_*`.
    interface IERC20Metadata {
        event Transfer(address indexed from, address indexed to, uint256 value);

        function balanceOf(address account) external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string memory);
        function name() external view returns (string memory);
    }

    /// Multicall3 `tryAggregate` (canonical contract, mds1/multicall).
    ///
    /// Returns `Result[]` — an array of `(bool success, bytes returnData)`
    /// structs (one per call), *not* two parallel arrays. Matches the
    /// canonical ABI at https://github.com/mds1/multicall.
    interface IMulticall3 {
        struct Call {
            address target;
            bytes callData;
        }

        struct Result {
            bool success;
            bytes returnData;
        }

        function tryAggregate(
            bool requireSuccess,
            Call[] calldata calls
        ) external payable returns (Result[] memory returnData);
    }
}
