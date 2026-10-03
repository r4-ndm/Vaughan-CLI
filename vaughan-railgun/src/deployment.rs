//! Which chains have RAILGUN, and where its contracts live.
//!
//! Mainnets come from railgun-rust's built-in table. Ethereum Sepolia is added
//! here with values verified on-chain on 2026-10-02 (see
//! `docs/railgun-go-no-go.md`). PulseChain has no RAILGUN deployment.

use alloy::primitives::{address, Address};
pub use broadcaster_core::deployment::RailgunDeployment;

/// Ethereum Sepolia chain id.
pub const SEPOLIA_CHAIN_ID: u64 = 11_155_111;

const SEPOLIA_RELAY_ADAPT: Address = address!("0x7e3d929EbD5bDC84d02Bd3205c777578f33A214D");
const SEPOLIA_RELAY_ADAPT_HISTORY: &[Address] = &[SEPOLIA_RELAY_ADAPT];

/// Block that created the Sepolia proxy; there is no legacy (pre-v2) history.
const SEPOLIA_DEPLOYMENT_BLOCK: u64 = 5_784_774;

const SEPOLIA: RailgunDeployment = RailgunDeployment {
    chain_id: SEPOLIA_CHAIN_ID,
    contract: address!("0xeCFCf3b4eC647c4Ca6D49108b311b7a7C9543fea"),
    relay_adapt_contract: SEPOLIA_RELAY_ADAPT,
    // No verified 7702 relay adapt on Sepolia; unused until Phase 3.
    relay_adapt_7702_contract: Address::ZERO,
    relay_adapt_history: SEPOLIA_RELAY_ADAPT_HISTORY,
    deployment_block: SEPOLIA_DEPLOYMENT_BLOCK,
    v2_start_block: SEPOLIA_DEPLOYMENT_BLOCK,
    legacy_shield_block: SEPOLIA_DEPLOYMENT_BLOCK,
};

/// RAILGUN deployment for `chain_id`, if there is one.
#[must_use]
pub const fn for_chain(chain_id: u64) -> Option<RailgunDeployment> {
    if chain_id == SEPOLIA_CHAIN_ID {
        return Some(SEPOLIA);
    }
    RailgunDeployment::for_chain(chain_id)
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
        for id in [1, 56, 137, 42_161, SEPOLIA_CHAIN_ID] {
            assert!(is_supported(id), "chain {id}");
        }
        for id in [369, 943, 31_337, 10, 8453] {
            assert!(!is_supported(id), "chain {id}");
        }
    }

    #[test]
    fn receive_and_shield_are_sepolia_only() {
        assert!(receive_enabled(SEPOLIA_CHAIN_ID));
        assert!(!receive_enabled(1));
        assert!(shield_enabled(SEPOLIA_CHAIN_ID));
        for id in [1, 56, 137, 42_161, 369] {
            assert!(!shield_enabled(id), "chain {id}");
        }
    }

    #[test]
    fn sepolia_scans_from_proxy_creation() {
        let d = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        assert_eq!(d.deployment_block, 5_784_774);
        assert_eq!(d.v2_start_block, d.deployment_block);
    }

    /// The approval-card humanizer in vaughan-core keeps its own address table.
    #[test]
    fn humanizer_contract_table_matches() {
        for id in [1, 56, 137, 42_161, SEPOLIA_CHAIN_ID] {
            let d = for_chain(id).unwrap();
            assert_eq!(
                vaughan_core::core::humanizer::railgun_contracts(id),
                Some((d.contract, d.relay_adapt_contract)),
                "chain {id}"
            );
        }
    }
}
