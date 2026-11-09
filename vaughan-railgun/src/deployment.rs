//! Which chains have RAILGUN, and where its contracts live.
//!
//! Mainnets and Ethereum Sepolia come from railgun-rust's built-in table.
//! PulseChain has no RAILGUN deployment.

use alloy::primitives::{address, Address};
pub use broadcaster_core::deployment::RailgunDeployment;

/// Ethereum Sepolia chain id.
pub const SEPOLIA_CHAIN_ID: u64 = 11_155_111;

/// Sepolia WETH address (wrapped base token).
pub const SEPOLIA_WETH: Address = address!("0xfff9976782d46cc05630d1f6ebab18b2324d6b14");

/// RAILGUN deployment for `chain_id`, if there is one.
#[must_use]
pub const fn for_chain(chain_id: u64) -> Option<RailgunDeployment> {
    RailgunDeployment::for_chain(chain_id)
}

/// Wrapped base token address (e.g. WETH, WBNB, WMATIC) for `chain_id` if supported.
#[must_use]
pub const fn wrapped_base_token(chain_id: u64) -> Option<Address> {
    match chain_id {
        SEPOLIA_CHAIN_ID => Some(SEPOLIA_WETH),
        1 => Some(address!("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2")),
        42_161 => Some(address!("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1")),
        56 => Some(address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c")),
        137 => Some(address!("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270")),
        8453 => Some(address!("0x4200000000000000000000000000000000000006")),
        _ => None,
    }
}

/// True when RAILGUN is deployed on `chain_id`.
#[must_use]
pub const fn is_supported(chain_id: u64) -> bool {
    for_chain(chain_id).is_some()
}

/// True when Vaughan shows the 0zk receive address on `chain_id`.
///
/// Sepolia only until Gate 3: Vaughan can't spend shielded funds yet, so it
/// doesn't invite real deposits on mainnets.
#[must_use]
pub const fn receive_enabled(chain_id: u64) -> bool {
    chain_id == SEPOLIA_CHAIN_ID
}

/// True when Vaughan offers shielding on `chain_id` (Sepolia only until
/// Gate 3, for the same reason as [`receive_enabled`]).
#[must_use]
pub const fn shield_enabled(chain_id: u64) -> bool {
    chain_id == SEPOLIA_CHAIN_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_chains() {
        for id in [1, 56, 137, 42_161, 8453, SEPOLIA_CHAIN_ID] {
            assert!(is_supported(id), "chain {id}");
            assert!(wrapped_base_token(id).is_some(), "wrapped base {id}");
        }
        for id in [369, 943, 31_337, 10] {
            assert!(!is_supported(id), "chain {id}");
            assert!(wrapped_base_token(id).is_none(), "wrapped base {id}");
        }
    }

    #[test]
    fn receive_and_shield_are_sepolia_only() {
        assert!(receive_enabled(SEPOLIA_CHAIN_ID));
        assert!(!receive_enabled(1));
        assert!(shield_enabled(SEPOLIA_CHAIN_ID));
        for id in [1, 56, 137, 42_161, 8453, 369] {
            assert!(!shield_enabled(id), "chain {id}");
        }
    }

    #[test]
    fn sepolia_scans_from_proxy_creation() {
        let d = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        assert_eq!(d.deployment_block, 5_784_774);
    }

    /// The approval-card humanizer in vaughan-core keeps its own address table.
    #[test]
    fn humanizer_contract_table_matches() {
        for id in [1, 56, 137, 42_161, SEPOLIA_CHAIN_ID] {
            let d = for_chain(id).unwrap();
            assert_eq!(
                vaughan_core::core::humanizer::railgun_contracts(id),
                Some((d.contract, d.relay_adapt_contract.expect("relay adapt"))),
                "chain {id}"
            );
        }
    }
}
