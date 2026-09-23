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
    static ETHW: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();
    static EMPTY: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();

    let build = |cid: u64| {
        write_allowed_addresses(cid)
            .map(|a| a.into_array())
            .collect()
    };

    match chain_id {
        369 => MAIN.get_or_init(|| build(369)),
        943 => TEST.get_or_init(|| build(943)),
        31337 => ANVIL.get_or_init(|| build(369)),
        10_001 => ETHW.get_or_init(|| build(10_001)),
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
///
/// On EthereumPoW this returns LFGswap's `WETHW` (the native wrap). Venues that
/// price against canonical WETH must use [`venue_wrapped_native`] instead.
pub fn wpls_for_chain(chain_id: u64) -> Option<Address> {
    match chain_id {
        369 => "0xA1077a294dDE1B09bB078844df40758a5D0f9a27".parse().ok(),
        943 => "0x70499adEBB11Efd915E3b69E700c331778628707".parse().ok(),
        // LFGswap WETH() — native wrap on EthereumPoW (not canonical WETH).
        10_001 => "0x7Bf88d2c0e32dE92Cdaf2D43CcDC23e8EdfD5990".parse().ok(),
        _ => None,
    }
}

/// Canonical WETH (pre-merge address) still used by UniWswap / PowSwap / Uniswap on ETHW.
const ETHW_CANONICAL_WETH: &str = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2";

/// Wrapped native token for a DEX venue's swap path on `chain_id`.
///
/// On Pulse this is always WPLS/tWPLS. On EthereumPoW, LFGswap uses WETHW while
/// UniWswap / PowSwap / Uniswap (Hedron) use canonical WETH.
pub fn venue_wrapped_native(venue: super::dex_catalog::DexVenue, chain_id: u64) -> Option<Address> {
    use super::dex_catalog::DexVenue;
    match (chain_id, venue) {
        (10_001, DexVenue::UniWswap | DexVenue::PowSwap | DexVenue::UniHedron) => {
            ETHW_CANONICAL_WETH.parse().ok()
        }
        _ => wpls_for_chain(chain_id),
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

    #[test]
    fn nine_inch_npm_allowed_on_369() {
        let npm = address!("0x18A532b36A9F6B10b3FEC5BF225C00A0Ec89B79E");
        assert!(is_allowed_dex_router(369, npm));
    }

    #[test]
    fn nine_mm_npm_allowed_on_369() {
        let npm = address!("0xCC05bf158202b4F461Ede8843d76dcd7Bbad07f2");
        assert!(is_allowed_dex_router(369, npm));
    }

    #[test]
    fn lfgswap_router_allowed_on_ethw() {
        let r = address!("0x4f381d5fF61ad1D0eC355fEd2Ac4000eA1e67854");
        assert!(is_allowed_dex_router(10_001, r));
        assert!(!is_allowed_dex_router(369, r));
        let npm = address!("0xC36442b4a4522E871399CD717aBDD847Ab11FE88");
        assert!(is_allowed_dex_router(10_001, npm));
    }

    #[test]
    fn venue_wrapped_native_ethw_splits_wethw_and_weth() {
        use super::super::dex_catalog::DexVenue;
        let wethw = wpls_for_chain(10_001).unwrap();
        assert_eq!(
            venue_wrapped_native(DexVenue::LfgSwap, 10_001).unwrap(),
            wethw
        );
        let weth = address!("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
        assert_eq!(
            venue_wrapped_native(DexVenue::UniWswap, 10_001).unwrap(),
            weth
        );
        assert_eq!(
            venue_wrapped_native(DexVenue::PowSwap, 10_001).unwrap(),
            weth
        );
        assert_eq!(
            venue_wrapped_native(DexVenue::PulseX, 369).unwrap(),
            wpls_for_chain(369).unwrap()
        );
    }
}
