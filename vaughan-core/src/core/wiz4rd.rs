//! wiz4rd-swap (Pancake V3 fork) deployment addresses for PulseChain.
//!
//! Source of truth mirrored from `wiz4rd-swap/docs/addresses.md` (deployed
//! 2026-08-20 on testnet 943). Mainnet (369) is not deployed yet.

use alloy::primitives::Address;

/// PulseChain testnet v4 chain id.
pub const WIZ4RD_TESTNET_CHAIN_ID: u64 = 943;

/// PulseChain mainnet chain id (no wiz4rd deploy yet).
pub const WIZ4RD_MAINNET_CHAIN_ID: u64 = 369;

/// PancakeV3PoolDeployer (943).
pub const POOL_DEPLOYER_943: &str = "0x55DC1d6155363CE68BB525ce473126a3d192574E";
/// PancakeV3Factory (943).
pub const FACTORY_943: &str = "0x297BeFB564d3Bba2D1913613B84Fb743C259C6cf";
/// SwapRouter (943).
pub const SWAP_ROUTER_943: &str = "0xfC656c95eCd418536844FeeaA46949bb9365BEaF";
/// NonfungiblePositionManager (943).
pub const POSITION_MANAGER_943: &str = "0xf1b1D004dD8bFC618F977F6ACAD127a60c566745";
/// QuoterV2 (943).
pub const QUOTER_V2_943: &str = "0x38d1752597c2c0BD25E980891cd6d74766138FB7";
/// TickLens (943).
pub const TICK_LENS_943: &str = "0xEE88CDf0D030d733A1E2a1fD9E6Ab3780DE7B768";
/// WPLS on PulseChain testnet v4.
pub const WPLS_943: &str = "0x70499adEBB11Efd915E3b69E700c331778628707";
/// Smoke ERC-20 WZRD (943).
pub const WZRD_SMOKE_943: &str = "0x29bab93456c0E97EE931C1554c7C215480aa7766";
/// Smoke pool WZRD/WPLS fee 500 (943).
pub const SMOKE_POOL_WZRD_WPLS_500_943: &str = "0xd47E01C1Af55a48C11d0E324fb1853cf2501e0Dc";

/// Fee tiers enabled on the wiz4rd factory (includes Pancake 2% = 20000).
pub const WIZ4RD_FEE_TIERS: &[u32] = &[100, 500, 2500, 10_000, 20_000];

/// Deployment snapshot for one chain (testnet today).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wiz4rdDeployment {
    pub chain_id: u64,
    pub factory: &'static str,
    pub pool_deployer: &'static str,
    pub swap_router: &'static str,
    pub position_manager: &'static str,
    pub quoter_v2: &'static str,
    pub tick_lens: &'static str,
    pub wpls: &'static str,
}

/// Live wiz4rd deploy on PulseChain testnet v4 (943).
pub const DEPLOYMENT_943: Wiz4rdDeployment = Wiz4rdDeployment {
    chain_id: WIZ4RD_TESTNET_CHAIN_ID,
    factory: FACTORY_943,
    pool_deployer: POOL_DEPLOYER_943,
    swap_router: SWAP_ROUTER_943,
    position_manager: POSITION_MANAGER_943,
    quoter_v2: QUOTER_V2_943,
    tick_lens: TICK_LENS_943,
    wpls: WPLS_943,
};

/// wiz4rd deployment for `chain_id`, if any.
pub fn deployment_for_chain(chain_id: u64) -> Option<&'static Wiz4rdDeployment> {
    match chain_id {
        WIZ4RD_TESTNET_CHAIN_ID => Some(&DEPLOYMENT_943),
        _ => None,
    }
}

/// Parse a static hex address used in this module.
pub fn parse_addr(s: &str) -> Option<Address> {
    s.parse().ok()
}

/// SwapRouter for `chain_id` when wiz4rd is deployed there.
pub fn swap_router(chain_id: u64) -> Option<Address> {
    deployment_for_chain(chain_id).and_then(|d| parse_addr(d.swap_router))
}

/// NonfungiblePositionManager for `chain_id` when wiz4rd is deployed there.
pub fn position_manager(chain_id: u64) -> Option<Address> {
    deployment_for_chain(chain_id).and_then(|d| parse_addr(d.position_manager))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn testnet_addresses_parse() {
        let d = deployment_for_chain(943).expect("943 deploy");
        assert!(parse_addr(d.swap_router).is_some());
        assert!(parse_addr(d.position_manager).is_some());
        assert!(parse_addr(d.factory).is_some());
        assert_eq!(
            swap_router(943).unwrap().to_string().to_lowercase(),
            SWAP_ROUTER_943.to_lowercase()
        );
    }

    #[test]
    fn mainnet_not_deployed_yet() {
        assert!(deployment_for_chain(369).is_none());
    }
}
