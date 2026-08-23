//! Allowlisted Uni-compatible DEX routers for Degen (and shared catalog).
//!
//! Addresses match the Dex TUI `venue_router` catalog (PulseX, 9mm, …).
//! Degen refuses to simulate/broadcast swaps to any other `to`.

use alloy::primitives::Address;
use std::collections::HashSet;
use std::sync::OnceLock;

/// `(chain_id, router)` pairs known safe for autonomous swaps.
const OFFICIAL_DEX_ROUTERS: &[(u64, &str)] = &[
    // PulseX
    (369, "0x165C3410fC91EF562C50559f7d2289fEbed552d9"), // V2
    (943, "0xDaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7"), // V2 testnet
    (369, "0xDA9aBA4eACF54E0273f56dfFee6B8F1e20B23Bba"), // V3
    (369, "0x98bf93ebf5c380C0e6Ae8e192A7e2AE08edAcc02"), // PulseX V1
    // 9mm
    (369, "0xcC73b59F8D7b7c532703bDfea2808a28a488cF47"),
    (369, "0x7bE8fbe502191bBBCb38b02f2d4fA0D628301bEA"),
    // 9inch
    (369, "0xeB45a3c4aedd0F47F345fB4c8A1802BB5740d725"),
    (369, "0x42556A17EF0Bd815bF21aD628DFd2e2f3b5F9ac7"),
    // SparkSwap
    (369, "0x76C08825b4A675FD6a17A244660BabeB4ADA79d5"),
    // Dextop / pDex
    (369, "0x1f849694Ef24a2245bCa415FE47500216B24d7FF"),
    (369, "0x1eC2eaA62117486c9b2a05F098a7bF2568e19204"),
    // Uni V3 Hedron
    (369, "0xE592427A0AEce92De3Edee1F18E0157C05861564"),
];

fn routers_for_chain(chain_id: u64) -> &'static HashSet<[u8; 20]> {
    // Cache per common chain; fall back to empty for unknown.
    static MAIN: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();
    static TEST: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();
    let build = |cid: u64| {
        OFFICIAL_DEX_ROUTERS
            .iter()
            .filter(|(c, _)| *c == cid)
            .filter_map(|(_, s)| s.parse::<Address>().ok())
            .map(|a| a.into_array())
            .collect()
    };
    match chain_id {
        369 => MAIN.get_or_init(|| build(369)),
        943 => TEST.get_or_init(|| build(943)),
        // Local Anvil often uses 31337 — accept Pulse mainnet catalog for CI plants.
        31337 => MAIN.get_or_init(|| build(369)),
        _ => {
            static EMPTY: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();
            EMPTY.get_or_init(HashSet::new)
        }
    }
}

/// True when `router` is a catalogued DEX router for `chain_id`.
pub fn is_allowed_dex_router(chain_id: u64, router: Address) -> bool {
    routers_for_chain(chain_id).contains(&router.into_array())
}

/// PulseX V2 mainnet — convenient Anvil plant / test target.
pub const PULSEX_V2_MAINNET: &str = "0x165C3410fC91EF562C50559f7d2289fEbed552d9";

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
}
