//! Allowlisted aggregator execution targets (`tx.to` / ERC-20 spender).
//!
//! Quote APIs return calldata + a router address. Before Vaughan signs, both
//! `to` and `spender` must appear on this list (same pattern as LibertySwap).

use alloy::primitives::Address;
use std::collections::HashSet;
use std::sync::OnceLock;

use crate::error::WalletError;

/// Known Brain / PulseSwap routers seen in fixtures and live Pulse quotes.
pub const OFFICIAL_AGG_ROUTERS: &[&str] = &[
    // SquirrelSwap Brain (api.squirrelswap.pro) — fixture + live
    "0xDa8953Fc615d6E816b9647Afd5536123dcE70B78",
    // PulseSwap advanced quote fixture / live shape
    "0xC994375187988C751C8fCb96A68A0f242947f0E6",
    // EmpX / EmpSeal on-chain router (PulseChain mainnet)
    "0x0Cf6D948Cf09ac83a6bf40C7AD7b44657A9F2A52",
    // Piteas public router (PulseChain mainnet)
    "0x6BF228eb7F8ad948d37deD07E595EfddfaAF88A6",
    // 9mm 9X unified API (`api.9mm.pro`) — execution router + ERC-20 allowance target
    "0xd5b775d1f15a864a5f6b94624e049cf758d013f7",
];

fn router_set() -> &'static HashSet<[u8; 20]> {
    static SET: OnceLock<HashSet<[u8; 20]>> = OnceLock::new();
    SET.get_or_init(|| {
        OFFICIAL_AGG_ROUTERS
            .iter()
            .filter_map(|s| s.parse::<Address>().ok())
            .map(|a| a.into_array())
            .collect()
    })
}

/// Chains where [`OFFICIAL_AGG_ROUTERS`] are deployed (PulseChain mainnet).
///
/// Anvil (`31337`) is included so local papers can reuse the same hex; live
/// aggregator APIs still target Pulse mainnet contracts.
pub fn agg_routers_supported_on(chain_id: u64) -> bool {
    matches!(chain_id, 369 | 31337)
}

/// True when `addr` is a known aggregator router / spender (any chain).
///
/// Prefer [`is_allowed_agg_router_on_chain`] at execution time so a Pulse
/// router address cannot be targeted on EthereumPoW / other nets.
pub fn is_allowed_agg_router(addr: Address) -> bool {
    router_set().contains(&addr.into_array())
}

/// True when `addr` is an allowlisted aggregator router **and** `chain_id`
/// is a PulseChain (or Anvil) network where those contracts live.
pub fn is_allowed_agg_router_on_chain(chain_id: u64, addr: Address) -> bool {
    agg_routers_supported_on(chain_id) && is_allowed_agg_router(addr)
}

/// Refuse quotes whose execution `to` or ERC-20 `spender` is not allowlisted
/// (Pulse catalog — callers that already scoped the venue to Pulse).
pub fn assert_agg_exec_targets(to: Address, spender: Address) -> Result<(), WalletError> {
    assert_agg_exec_targets_on_chain(369, to, spender)
}

/// Like [`assert_agg_exec_targets`], but refuses when `chain_id` is not a
/// Pulse / Anvil network that hosts the catalogued routers.
pub fn assert_agg_exec_targets_on_chain(
    chain_id: u64,
    to: Address,
    spender: Address,
) -> Result<(), WalletError> {
    if !agg_routers_supported_on(chain_id) {
        return Err(WalletError::InvalidTransaction(format!(
            "aggregator: routers are PulseChain mainnet only — refusing on chain {chain_id}"
        )));
    }
    if !is_allowed_agg_router(to) {
        return Err(WalletError::InvalidTransaction(format!(
            "aggregator: router {:#x} not on allowlist — refusing to quote",
            to
        )));
    }
    if !is_allowed_agg_router(spender) {
        return Err(WalletError::InvalidTransaction(format!(
            "aggregator: spender {:#x} not on allowlist — refusing to quote",
            spender
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn squirrel_pulseswap_empx_piteas_nine_mm_routers_allowed() {
        assert!(is_allowed_agg_router(address!(
            "0xDa8953Fc615d6E816b9647Afd5536123dcE70B78"
        )));
        assert!(is_allowed_agg_router(address!(
            "0xC994375187988C751C8fCb96A68A0f242947f0E6"
        )));
        assert!(is_allowed_agg_router(address!(
            "0x0Cf6D948Cf09ac83a6bf40C7AD7b44657A9F2A52"
        )));
        assert!(is_allowed_agg_router(address!(
            "0x6BF228eb7F8ad948d37deD07E595EfddfaAF88A6"
        )));
        assert!(is_allowed_agg_router(address!(
            "0xd5b775d1f15a864a5f6b94624e049cf758d013f7"
        )));
    }

    #[test]
    fn unknown_router_rejected() {
        let evil = address!("0x1111111111111111111111111111111111111111");
        let ok = address!("0xDa8953Fc615d6E816b9647Afd5536123dcE70B78");
        assert!(assert_agg_exec_targets(evil, evil).is_err());
        assert!(assert_agg_exec_targets(ok, evil).is_err());
        assert!(assert_agg_exec_targets(ok, ok).is_ok());
    }

    #[test]
    fn agg_routers_refused_off_pulse() {
        let ok = address!("0xDa8953Fc615d6E816b9647Afd5536123dcE70B78");
        assert!(is_allowed_agg_router_on_chain(369, ok));
        assert!(!is_allowed_agg_router_on_chain(10_001, ok));
        assert!(!is_allowed_agg_router_on_chain(943, ok));
        assert!(assert_agg_exec_targets_on_chain(10_001, ok, ok).is_err());
        assert!(assert_agg_exec_targets_on_chain(369, ok, ok).is_ok());
    }
}
