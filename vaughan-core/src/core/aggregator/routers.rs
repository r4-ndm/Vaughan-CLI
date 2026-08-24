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

/// True when `addr` is a known aggregator router / spender.
pub fn is_allowed_agg_router(addr: Address) -> bool {
    router_set().contains(&addr.into_array())
}

/// Refuse quotes whose execution `to` or ERC-20 `spender` is not allowlisted.
pub fn assert_agg_exec_targets(to: Address, spender: Address) -> Result<(), WalletError> {
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
    fn squirrel_and_pulseswap_routers_allowed() {
        assert!(is_allowed_agg_router(address!(
            "0xDa8953Fc615d6E816b9647Afd5536123dcE70B78"
        )));
        assert!(is_allowed_agg_router(address!(
            "0xC994375187988C751C8fCb96A68A0f242947f0E6"
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
}
