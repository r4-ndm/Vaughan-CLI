//! pHEX / eHEX contract identity for stake reads.

use alloy::primitives::{address, Address};

/// PulseChain state-fork HEX (same address as Ethereum HEX) — stakeable.
pub fn phex_address() -> Address {
    address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39")
}

/// Bridged HEX on PulseChain — ERC-20 only, no stake views.
pub fn ehex_address() -> Address {
    address!("0x57fde0a71132198BBeC939B98976993d8D89D225")
}

/// Which HEX contract a tool/UI resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexContractKind {
    Phex,
    Ehex,
    Custom,
}

/// Resolved contract reference for stake tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexContractRef {
    pub address: Address,
    pub kind: HexContractKind,
    pub label: &'static str,
    pub supports_staking: bool,
    pub note: &'static str,
}

/// Resolve `phex` / `ehex` / `0x…` for stake tools (pure).
pub fn resolve_hex_contract(which: &str) -> Result<HexContractRef, String> {
    let key = which.trim().to_ascii_lowercase();
    if matches!(key.as_str(), "phex" | "hex" | "ph" | "") {
        return Ok(HexContractRef {
            address: phex_address(),
            kind: HexContractKind::Phex,
            label: "pHEX",
            supports_staking: true,
            note: "pHEX is the PulseChain state-fork HEX at the original Ethereum HEX address. \
                   Stake state lives here. Distinct from bridged eHEX.",
        });
    }
    if matches!(key.as_str(), "ehex" | "bridged" | "bridged_hex") {
        return Ok(HexContractRef {
            address: ehex_address(),
            kind: HexContractKind::Ehex,
            label: "eHEX",
            supports_staking: false,
            note: "eHEX is HEX bridged from Ethereum (ERC-20). It does not expose HEX stake \
                   views (currentDay/stakeLists). Use contract=phex for stake reads.",
        });
    }
    let addr: Address = key
        .parse()
        .map_err(|_| {
            format!(
                "Invalid HEX contract selector \"{which}\". Use phex, ehex, or 0x address."
            )
        })?;
    if addr == phex_address() {
        return resolve_hex_contract("phex");
    }
    if addr == ehex_address() {
        return resolve_hex_contract("ehex");
    }
    Ok(HexContractRef {
        address: addr,
        kind: HexContractKind::Custom,
        label: "custom",
        supports_staking: true,
        note: "Custom address — stake views are attempted; may revert if not HEX-compatible.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_phex_aliases_and_ehex() {
        assert_eq!(resolve_hex_contract("HEX").unwrap().kind, HexContractKind::Phex);
        assert!(!resolve_hex_contract("ehex").unwrap().supports_staking);
        assert_eq!(
            resolve_hex_contract(&format!("{:#x}", phex_address()))
                .unwrap()
                .kind,
            HexContractKind::Phex
        );
    }
}
