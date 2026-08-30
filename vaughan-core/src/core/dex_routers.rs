//! Allowlisted Uni-compatible DEX contracts for Sentient (and shared catalog).
//!
//! Addresses are derived from [`super::dex_catalog`] — do not duplicate hex here.

use alloy::primitives::Address;
use std::collections::HashSet;
use std::sync::OnceLock;

use super::dex_catalog::write_allowed_addresses;

fn routers_for_chain(chain_id: u64) -> &'static HashSet<[u8; 20]> {
    static MAIN: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();
    static TEST: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();
    static ANVIL: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();
    static EMPTY: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();

    let build = |cid: u64| write_allowed_addresses(cid).map(|a| a.into_array()).collect();

    match chain_id {
        369 => MAIN.get_or_init(|| build(369)),
        943 => TEST.get_or_init(|| build(943)),
        31337 => ANVIL.get_or_init(|| build(369)),
        _ => EMPTY.get_or_init(HashSet::new),
    }
}

/// True when `router` is a catalogued DEX SwapRouter or PositionManager for `chain_id`.
pub fn is_allowed_dex_router(chain_id: u64, router: Address) -> bool {
    routers_for_chain(chain_id).contains(&router.into_array())
}

/// PulseX V2 mainnet — convenient Anvil plant / test target.
pub const PULSEX_V2_MAINNET: &str = "0x165C3410fC91EF562C50559f7d2289fEbed552d9";

/// Pulse wrapped native (WPLS / tWPLS) for wrap/unwrap flows.
pub fn wpls_for_chain(chain_id: u64) -> Option<Address> {
    match chain_id {
        369 => "0xA1077a294dDE1B09bB078844df40758a5D0f9a27".parse().ok(),
        943 => "0x70499adEBB11Efd915E3b69E700c331778628707".parse().ok(),
        _ => None,
    }
}

/// Catalogued DEX write targets for `chain_id` (label = short venue hint).
pub fn dex_routers_labeled(chain_id: u64) -> Vec<(Address, &'static str)> {
    write_allowed_addresses(chain_id)
        .map(|a| (a, "DEX"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn pulsex_v2_allowed_on_369_and_anvil() {
        let r = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9");
        assert!(is_allowed_dex_router(369, r));
        assert!(is_allowed_dex_router(31337, r));
        assert!(!is_allowed_dex_router(
            369,
            address!("0x1111111111111111111111111111111111111111")
        ));
    }

    #[test]
    fn wiz4rd_swap_router_allowed_on_943() {
        let r = address!("0xfC656c95eCd418536844FeeaA46949bb9365BEaF");
        assert!(is_allowed_dex_router(943, r));
        assert!(!is_allowed_dex_router(369, r));
    }

    #[test]
    fn wiz4rd_npm_allowed_on_943() {
        let npm = address!("0xf1b1D004dD8bFC618F977F6ACAD127a60c566745");
        assert!(is_allowed_dex_router(943, npm));
    }
}
