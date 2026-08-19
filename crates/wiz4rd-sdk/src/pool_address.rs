//! PancakeSwap V3 pool address derivation.
//!
//! Mirrors `PoolAddress.sol` from `pancakeswap/pancake-v3-contracts`
//! (`projects/v3-periphery/contracts/libraries/PoolAddress.sol`):
//!
//! ```text
//! pool = keccak256(
//!     abi.encodePacked(
//!         hex"ff",
//!         deployer,                      // PancakeV3PoolDeployer (NOT factory)
//!         keccak256(abi.encode(token0, token1, fee)),
//!         POOL_INIT_CODE_HASH            // keccak256(PancakeV3Pool creation code)
//!     )
//! )[12:]
//! ```
//!
//! The `deployer` is the `PancakeV3PoolDeployer` contract address. On the
//! PancakeSwap fork the factory delegates pool creation to this deployer, so
//! deriving with the factory address produces the wrong pool address.

use alloy::primitives::{b256, keccak256, Address, B256};
use alloy::sol_types::SolValue;

/// `POOL_INIT_CODE_HASH` from periphery `PoolAddress.sol`. This is
/// `keccak256(PancakeV3Pool creation code)` under the fork's pinned compiler
/// settings. Verify after compiling the fork — see `docs/preflight.md` §6.
pub const POOL_INIT_CODE_HASH: B256 =
    b256!("6ce8eb472fa82df5469c6ab6d485f17c3ad13c8cd7af59b3d4a8026c5ce0f7e2");

/// The identifying key of a pool: ordered tokens + fee tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
}

/// Order a token pair into `(token0, token1)` per `PoolAddress.getPoolKey`
/// (token0 = the numerically smaller address).
pub fn get_pool_key(token_a: Address, token_b: Address, fee: u32) -> PoolKey {
    if token_a < token_b {
        PoolKey { token0: token_a, token1: token_b, fee }
    } else {
        PoolKey { token0: token_b, token1: token_a, fee }
    }
}

/// Deterministically compute the PancakeSwap V3 pool address.
///
/// `deployer` must be the **`PancakeV3PoolDeployer`** address (the factory's
/// pool-deployer), not the factory itself.
pub fn compute_pool_address(deployer: Address, key: PoolKey) -> Address {
    let salt = keccak256((key.token0, key.token1, key.fee).abi_encode());
    // keccak256(0xff ++ deployer ++ salt ++ POOL_INIT_CODE_HASH)[12:]
    // `Address::create2` packs 0xff as a single byte (canonical CREATE2).
    deployer.create2(salt, POOL_INIT_CODE_HASH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn addr(s: &str) -> Address {
        Address::from_str(s).unwrap()
    }

    #[test]
    fn get_pool_key_orders_by_address() {
        let a = addr("0x1111111111111111111111111111111111111111");
        let b = addr("0x2222222222222222222222222222222222222222");
        assert_eq!(get_pool_key(a, b, 500).token0, a);
        assert_eq!(get_pool_key(b, a, 500).token0, a);
        assert_eq!(get_pool_key(b, a, 500).token1, b);
    }

    #[test]
    fn get_pool_key_keeps_fee() {
        let a = addr("0x1111111111111111111111111111111111111111");
        let b = addr("0x2222222222222222222222222222222222222222");
        assert_eq!(get_pool_key(a, b, 2500).fee, 2500);
    }

    /// Cross-check against a real on-chain PancakeSwap V3 pool on BSC mainnet:
    /// WBNB / USDT, fee tier 500. Pool address + pool deployer both verified
    /// live via `cast` against the BSC factory (2026-08-18):
    ///   factory.getPool(WBNB, USDT, 500) -> 0x36696169C63e42cd08ce11f5deeBbCeBae652050
    ///   factory.poolDeployer()            -> 0x41ff9AA7e16B8B1a8a8dc4f0eFacd93D02d071c9
    #[test]
    fn bsc_mainnet_wbnb_usdt_fee500_matches_onchain_pool() {
        let pool_deployer = addr("0x41ff9AA7e16B8B1a8a8dc4f0eFacd93D02d071c9"); // PancakeV3PoolDeployer (BSC)
        let wbnb = addr("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
        let usdt = addr("0x55d398326f99059fF775485246999027B3197955");

        let key = get_pool_key(wbnb, usdt, 500);
        let derived = compute_pool_address(pool_deployer, key);

        // Real pool address on BSC mainnet (from factory.getPool).
        let expected = addr("0x36696169C63e42cd08ce11f5deeBbCeBae652050");
        assert_eq!(derived, expected, "derived pool address must match on-chain pool");
    }
}
