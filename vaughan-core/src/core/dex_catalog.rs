//! PulseChain DEX venue catalog — single source for routers, NPM, and allowlists.
//!
//! Dex TUI, MCP propose tools, and Sentient gates import from here so addresses
//! never drift between UI picker and `is_allowed_dex_router`.

use alloy::primitives::Address;
use std::str::FromStr;

use super::wiz4rd::{FACTORY_943, POSITION_MANAGER_943, SWAP_ROUTER_943};

/// Uni V2-style router vs V3 SwapRouter periphery.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum DexProtocol {
    V2,
    V3,
}

impl DexProtocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::V2 => "V2",
            Self::V3 => "V3",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::V2 => Self::V3,
            Self::V3 => Self::V2,
        }
    }
}

/// PulseChain DEX venues. AMM Uni-forks have catalogued routers; OTC/Balancer listed only.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum DexVenue {
    Wiz4rd,
    PulseX,
    PulseXV1,
    NineMm,
    NineInch,
    SparkSwap,
    Dextop,
    UniHedron,
    PDex,
    Phux,
    Tide,
    FiDex,
    Bistro,
    AgoraX,
    Curv,
    Custom,
}

/// ↑/↓ venue picker order (matches legacy Dex TUI).
pub const DEX_VENUES: &[DexVenue] = &[
    DexVenue::Wiz4rd,
    DexVenue::PulseX,
    DexVenue::PulseXV1,
    DexVenue::NineMm,
    DexVenue::NineInch,
    DexVenue::SparkSwap,
    DexVenue::Dextop,
    DexVenue::UniHedron,
    DexVenue::PDex,
    DexVenue::Phux,
    DexVenue::Tide,
    DexVenue::FiDex,
    DexVenue::Bistro,
    DexVenue::AgoraX,
    DexVenue::Curv,
    DexVenue::Custom,
];

impl DexVenue {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wiz4rd => "Wiz4rd",
            Self::PulseX => "PulseX",
            Self::PulseXV1 => "PulseX V1",
            Self::NineMm => "9mm",
            Self::NineInch => "9inch",
            Self::SparkSwap => "SparkSwap",
            Self::Dextop => "Dextop",
            Self::UniHedron => "Uniswap",
            Self::PDex => "pDex",
            Self::Phux => "PHUX",
            Self::Tide => "0xTide",
            Self::FiDex => "FiDex",
            Self::Bistro => "0xBistro",
            Self::AgoraX => "AgoraX",
            Self::Curv => "CURV",
            Self::Custom => "Custom",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::Wiz4rd => "Vaughan Pancake V3 fork · Pulse testnet 943",
            Self::PulseX => "largest PLS DEX · V2 AMM + V3 SwapRouter",
            Self::PulseXV1 => "legacy PulseX V1 AMM router",
            Self::NineMm => "Uni V3-fork concentrated liquidity",
            Self::NineInch => "V2 + V3 DEX (limit orders on site)",
            Self::SparkSwap => "dexSWAP / Spark Swap (V2-style)",
            Self::Dextop => "Uni V3-style · zkzx frontend",
            Self::UniHedron => "Uniswap V3 periphery on PulseChain",
            Self::PDex => "pDex V3 router",
            Self::Phux => "Balancer-style weighted pools — not wired",
            Self::Tide => "Balancer-fork dynamic fees — not wired",
            Self::FiDex => "Function Island — paste router (unknown)",
            Self::Bistro => "OTC — not AMM swap yet",
            Self::AgoraX => "OTC marketplace — not AMM swap yet",
            Self::Curv => "OTC + aggregator — not AMM swap yet",
            Self::Custom => "paste any Uni V2/V3-compatible router",
        }
    }

    pub fn unsupported_reason(self) -> Option<&'static str> {
        match self {
            Self::Phux => Some("Balancer vault — need Balancer swap path"),
            Self::Tide => Some("Balancer-fork — need vault swap path"),
            Self::Bistro | Self::AgoraX | Self::Curv => Some("OTC desk — not an AMM router"),
            Self::FiDex => Some("no published router in catalog yet"),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        let i = DEX_VENUES.iter().position(|v| *v == self).unwrap_or(0);
        DEX_VENUES[(i + 1) % DEX_VENUES.len()]
    }

    pub fn prev(self) -> Self {
        let i = DEX_VENUES.iter().position(|v| *v == self).unwrap_or(0);
        DEX_VENUES[(i + DEX_VENUES.len() - 1) % DEX_VENUES.len()]
    }
}

/// On-chain contract role within a venue deploy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DexContractRole {
    SwapRouter,
    PositionManager,
    /// Pancake/Uni V3 factory (`getPool` reads).
    V3Factory,
    /// Uni V2 factory (`getPair` reads).
    V2Factory,
    /// Read-only quote helper (`eth_call` only — not in write allowlist).
    QuoterV2,
}

struct CatalogEntry {
    venue: DexVenue,
    chain_id: u64,
    protocol: Option<DexProtocol>,
    role: DexContractRole,
    address: &'static str,
}

/// Verified deploy facts — keep in sync with block explorers / venue docs.
const CATALOG: &[CatalogEntry] = &[
    // wiz4rd-swap (943)
    CatalogEntry {
        venue: DexVenue::Wiz4rd,
        chain_id: 943,
        protocol: Some(DexProtocol::V3),
        role: DexContractRole::SwapRouter,
        address: SWAP_ROUTER_943,
    },
    CatalogEntry {
        venue: DexVenue::Wiz4rd,
        chain_id: 943,
        protocol: None,
        role: DexContractRole::PositionManager,
        address: POSITION_MANAGER_943,
    },
    CatalogEntry {
        venue: DexVenue::Wiz4rd,
        chain_id: 943,
        protocol: None,
        role: DexContractRole::V3Factory,
        address: FACTORY_943,
    },
    // PulseX
    CatalogEntry {
        venue: DexVenue::PulseX,
        chain_id: 369,
        protocol: Some(DexProtocol::V2),
        role: DexContractRole::SwapRouter,
        address: "0x165C3410fC91EF562C50559f7d2289fEbed552d9",
    },
    CatalogEntry {
        venue: DexVenue::PulseX,
        chain_id: 943,
        protocol: Some(DexProtocol::V2),
        role: DexContractRole::SwapRouter,
        address: "0xDaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7",
    },
    CatalogEntry {
        venue: DexVenue::PulseX,
        chain_id: 369,
        protocol: Some(DexProtocol::V3),
        role: DexContractRole::SwapRouter,
        address: "0xDA9aBA4eACF54E0273f56dfFee6B8F1e20B23Bba",
    },
    CatalogEntry {
        venue: DexVenue::PulseXV1,
        chain_id: 369,
        protocol: Some(DexProtocol::V2),
        role: DexContractRole::SwapRouter,
        address: "0x98bf93ebf5c380C0e6Ae8e192A7e2AE08edAcc02",
    },
    // 9mm
    CatalogEntry {
        venue: DexVenue::NineMm,
        chain_id: 369,
        protocol: Some(DexProtocol::V2),
        role: DexContractRole::SwapRouter,
        address: "0xcC73b59F8D7b7c532703bDfea2808a28a488cF47",
    },
    CatalogEntry {
        venue: DexVenue::NineMm,
        chain_id: 369,
        protocol: Some(DexProtocol::V3),
        role: DexContractRole::SwapRouter,
        address: "0x7bE8fbe502191bBBCb38b02f2d4fA0D628301bEA",
    },
    CatalogEntry {
        venue: DexVenue::NineMm,
        chain_id: 369,
        protocol: None,
        role: DexContractRole::PositionManager,
        address: "0xCC05bf158202b4F461Ede8843d76dcd7Bbad07f2",
    },
    CatalogEntry {
        venue: DexVenue::NineMm,
        chain_id: 369,
        protocol: None,
        role: DexContractRole::QuoterV2,
        address: "0xd6840a5f07d21e68383f159a19a9842af32bdcc5",
    },
    CatalogEntry {
        venue: DexVenue::NineMm,
        chain_id: 369,
        protocol: None,
        role: DexContractRole::V3Factory,
        address: "0xe50DbDC88E87a2C92984d794bcF3D1d76f619C68",
    },
    // 9inch
    CatalogEntry {
        venue: DexVenue::NineInch,
        chain_id: 369,
        protocol: Some(DexProtocol::V2),
        role: DexContractRole::V2Factory,
        address: "0x5b9F077A77db37F3Be0A5b5d31BAeff4bc5C0bD7",
    },
    CatalogEntry {
        venue: DexVenue::NineInch,
        chain_id: 369,
        protocol: Some(DexProtocol::V2),
        role: DexContractRole::SwapRouter,
        address: "0xeB45a3c4aedd0F47F345fB4c8A1802BB5740d725",
    },
    CatalogEntry {
        venue: DexVenue::NineInch,
        chain_id: 369,
        protocol: Some(DexProtocol::V3),
        role: DexContractRole::SwapRouter,
        address: "0x42556A17EF0Bd815bF21aD628DFd2e2f3b5F9ac7",
    },
    CatalogEntry {
        venue: DexVenue::NineInch,
        chain_id: 369,
        protocol: None,
        role: DexContractRole::V3Factory,
        address: "0xCfd33C867C9F031AadfF7939Cb8086Ee5ae88c41",
    },
    CatalogEntry {
        venue: DexVenue::NineInch,
        chain_id: 369,
        protocol: None,
        role: DexContractRole::PositionManager,
        address: "0x18A532b36A9F6B10b3FEC5BF225C00A0Ec89B79E",
    },
    // SparkSwap
    CatalogEntry {
        venue: DexVenue::SparkSwap,
        chain_id: 369,
        protocol: Some(DexProtocol::V2),
        role: DexContractRole::SwapRouter,
        address: "0x76C08825b4A675FD6a17A244660BabeB4ADA79d5",
    },
    // Dextop / pDex V3
    CatalogEntry {
        venue: DexVenue::Dextop,
        chain_id: 369,
        protocol: Some(DexProtocol::V3),
        role: DexContractRole::SwapRouter,
        address: "0x1f849694Ef24a2245bCa415FE47500216B24d7FF",
    },
    CatalogEntry {
        venue: DexVenue::PDex,
        chain_id: 369,
        protocol: Some(DexProtocol::V3),
        role: DexContractRole::SwapRouter,
        address: "0x1eC2eaA62117486c9b2a05F098a7bF2568e19204",
    },
    // Uni V3 Hedron
    CatalogEntry {
        venue: DexVenue::UniHedron,
        chain_id: 369,
        protocol: Some(DexProtocol::V3),
        role: DexContractRole::SwapRouter,
        address: "0xE592427A0AEce92De3Edee1F18E0157C05861564",
    },
];

fn parse_static_addr(s: &str) -> Option<Address> {
    Address::from_str(s).ok()
}

pub fn chain_label(chain_id: u64) -> &'static str {
    match chain_id {
        369 => "PulseChain mainnet",
        943 => "PulseChain testnet",
        _ => "this network",
    }
}

/// Stable slug for MCP / agent tool args (e.g. `wiz4rd`, `9mm`).
pub fn venue_slug(venue: DexVenue) -> &'static str {
    match venue {
        DexVenue::Wiz4rd => "wiz4rd",
        DexVenue::PulseX => "pulsex",
        DexVenue::PulseXV1 => "pulsex_v1",
        DexVenue::NineMm => "9mm",
        DexVenue::NineInch => "9inch",
        DexVenue::SparkSwap => "sparkswap",
        DexVenue::Dextop => "dextop",
        DexVenue::UniHedron => "uniswap",
        DexVenue::PDex => "pdex",
        DexVenue::Phux => "phux",
        DexVenue::Tide => "0xtide",
        DexVenue::FiDex => "fidex",
        DexVenue::Bistro => "0xbistro",
        DexVenue::AgoraX => "agorax",
        DexVenue::Curv => "curv",
        DexVenue::Custom => "custom",
    }
}

fn normalize_venue_label(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Parse a venue slug or label from MCP / agent args.
pub fn parse_dex_venue_label(raw: &str) -> Option<DexVenue> {
    let key = normalize_venue_label(raw);
    if key.is_empty() {
        return None;
    }
    for &venue in DEX_VENUES {
        let slug = normalize_venue_label(venue_slug(venue));
        let label = normalize_venue_label(venue.label());
        if key == slug || key == label {
            return Some(venue);
        }
    }
    match key.as_str() {
        "ninemm" | "nine_mm" => Some(DexVenue::NineMm),
        "nineinch" | "nine_inch" => Some(DexVenue::NineInch),
        "pulsexv1" | "pulsexlegacy" => Some(DexVenue::PulseXV1),
        _ => None,
    }
}

/// Swap router for `(venue, protocol, chain)` when catalogued and supported.
pub fn venue_swap_router(venue: DexVenue, protocol: DexProtocol, chain_id: u64) -> Option<Address> {
    if venue.unsupported_reason().is_some() || venue == DexVenue::Custom {
        return None;
    }
    CATALOG.iter().find_map(|e| {
        if e.venue == venue
            && e.chain_id == chain_id
            && e.protocol == Some(protocol)
            && e.role == DexContractRole::SwapRouter
        {
            parse_static_addr(e.address)
        } else {
            None
        }
    })
}

/// NPM for concentrated LP when catalogued (wiz4rd 943; 9mm 369).
pub fn venue_position_manager(venue: DexVenue, chain_id: u64) -> Option<Address> {
    CATALOG.iter().find_map(|e| {
        if e.venue == venue && e.chain_id == chain_id && e.role == DexContractRole::PositionManager
        {
            parse_static_addr(e.address)
        } else {
            None
        }
    })
}

/// V3 factory for pool reads when catalogued.
pub fn venue_v3_factory(venue: DexVenue, chain_id: u64) -> Option<Address> {
    CATALOG.iter().find_map(|e| {
        if e.venue == venue && e.chain_id == chain_id && e.role == DexContractRole::V3Factory {
            parse_static_addr(e.address)
        } else {
            None
        }
    })
}

/// V2 factory for pair reads when catalogued (9inch 369).
pub fn venue_v2_factory(venue: DexVenue, chain_id: u64) -> Option<Address> {
    CATALOG.iter().find_map(|e| {
        if e.venue == venue && e.chain_id == chain_id && e.role == DexContractRole::V2Factory {
            parse_static_addr(e.address)
        } else {
            None
        }
    })
}

/// QuoterV2 for browserless V3 quotes when catalogued (9mm 369 today).
pub fn venue_quoter_v2(venue: DexVenue, chain_id: u64) -> Option<Address> {
    CATALOG.iter().find_map(|e| {
        if e.venue == venue && e.chain_id == chain_id && e.role == DexContractRole::QuoterV2 {
            parse_static_addr(e.address)
        } else {
            None
        }
    })
}

/// All write-gated DEX contract addresses for `chain_id` (routers, NPM, V3 factories).
pub fn write_allowed_addresses(chain_id: u64) -> impl Iterator<Item = Address> {
    let effective = if chain_id == 31337 { 369 } else { chain_id };
    CATALOG.iter().filter_map(move |e| {
        if e.chain_id != effective {
            return None;
        }
        match e.role {
            DexContractRole::SwapRouter
            | DexContractRole::PositionManager
            | DexContractRole::V3Factory => parse_static_addr(e.address),
            DexContractRole::V2Factory | DexContractRole::QuoterV2 => None,
        }
    })
}

/// Venues with a catalogued NPM on `chain_id` (wiz4rd 943 focus).
pub fn lp_v3_venues(chain_id: u64) -> impl Iterator<Item = DexVenue> {
    DEX_VENUES
        .iter()
        .copied()
        .filter(move |venue| venue_position_manager(*venue, chain_id).is_some())
}

/// TUI ↑/↓ venue picker order. Includes Wiz4rd on mainnet as a hint-only slot
/// (NPM lives on testnet 943 — see [`venue_position_manager`]).
pub fn lp_v3_venue_picker(chain_id: u64) -> Vec<DexVenue> {
    match chain_id {
        369 => vec![DexVenue::NineInch, DexVenue::NineMm, DexVenue::Wiz4rd],
        943 => vec![DexVenue::Wiz4rd],
        _ => lp_v3_venues(chain_id).collect(),
    }
}

/// V2 LP venue for `chain_id` (9inch on 369 only today).
pub fn lp_v2_venue(chain_id: u64) -> Option<DexVenue> {
    if chain_id == 369 && venue_v2_factory(DexVenue::NineInch, chain_id).is_some() {
        return Some(DexVenue::NineInch);
    }
    None
}

/// Which LP stack the TUI should show: wiz4rd V3 on 943, 9inch V3 on 369.
pub fn lp_stack_for_chain(chain_id: u64) -> Option<LpStack> {
    if chain_id == 943 && venue_position_manager(DexVenue::Wiz4rd, chain_id).is_some() {
        return Some(LpStack::V3 {
            venue: DexVenue::Wiz4rd,
        });
    }
    if chain_id == 369 && venue_position_manager(DexVenue::NineInch, chain_id).is_some() {
        return Some(LpStack::V3 {
            venue: DexVenue::NineInch,
        });
    }
    if let Some(venue) = lp_v2_venue(chain_id) {
        return Some(LpStack::V2 { venue });
    }
    lp_v3_venues(chain_id)
        .next()
        .map(|venue| LpStack::V3 { venue })
}

/// Browserless LP mode for the active chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LpStack {
    V3 { venue: DexVenue },
    V2 { venue: DexVenue },
}

impl LpStack {
    pub fn venue(self) -> DexVenue {
        match self {
            Self::V3 { venue } | Self::V2 { venue } => venue,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::V3 { .. } => "V3 CL",
            Self::V2 { .. } => "V2 AMM",
        }
    }
}

/// Default V3 LP venue (wiz4rd 943, 9inch 369).
pub fn default_lp_v3_venue(chain_id: u64) -> Option<DexVenue> {
    match chain_id {
        943 => venue_position_manager(DexVenue::Wiz4rd, chain_id).map(|_| DexVenue::Wiz4rd),
        369 => venue_position_manager(DexVenue::NineInch, chain_id).map(|_| DexVenue::NineInch),
        _ => lp_v3_venues(chain_id).next(),
    }
}

/// Default LP venue for TUI stack picker.
pub fn default_lp_venue(chain_id: u64) -> Option<DexVenue> {
    lp_stack_for_chain(chain_id).map(|s| s.venue())
}

/// Human hint when no router is catalogued for the picker selection.
pub fn missing_router_hint(venue: DexVenue, protocol: DexProtocol, chain_id: u64) -> String {
    if let Some(why) = venue.unsupported_reason() {
        return format!("{} — {} · {}", venue.label(), venue.blurb(), why);
    }
    let other = protocol.toggle();
    let other_ok = venue_swap_router(venue, other, chain_id).is_some();
    let mainnet_ok = chain_id != 369 && venue_swap_router(venue, protocol, 369).is_some();
    let mut parts = vec![format!(
        "{} {} — no catalogued router on {}",
        venue.label(),
        protocol.label(),
        chain_label(chain_id)
    )];
    if other_ok {
        parts.push(format!("try ←/→ for {}", other.label()));
    }
    if mainnet_ok {
        parts.push("or Settings→Net → PulseChain mainnet".into());
    }
    parts.push("Custom = paste any Uni V2/V3 router".into());
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn venue_cycle_order() {
        assert_eq!(DexVenue::Wiz4rd.next(), DexVenue::PulseX);
        assert_eq!(DexVenue::Custom.next(), DexVenue::Wiz4rd);
        assert_eq!(DEX_VENUES.len(), 16);
    }

    #[test]
    fn pulsex_v2_v3_mainnet() {
        assert!(venue_swap_router(DexVenue::PulseX, DexProtocol::V2, 369).is_some());
        assert_eq!(
            venue_swap_router(DexVenue::PulseX, DexProtocol::V3, 369),
            Some(address!("0xDA9aBA4eACF54E0273f56dfFee6B8F1e20B23Bba"))
        );
    }

    #[test]
    fn nine_mm_v3_mainnet_only() {
        assert!(venue_swap_router(DexVenue::NineMm, DexProtocol::V3, 369).is_some());
        assert!(venue_swap_router(DexVenue::NineMm, DexProtocol::V3, 943).is_none());
    }

    #[test]
    fn nine_mm_npm_and_quoter_on_369() {
        assert_eq!(
            venue_position_manager(DexVenue::NineMm, 369),
            Some(address!("0xCC05bf158202b4F461Ede8843d76dcd7Bbad07f2"))
        );
        assert_eq!(
            venue_quoter_v2(DexVenue::NineMm, 369),
            Some(address!("0xd6840a5f07d21e68383f159a19a9842af32bdcc5"))
        );
        assert_eq!(
            venue_v3_factory(DexVenue::NineMm, 369),
            Some(address!("0xe50DbDC88E87a2C92984d794bcF3D1d76f619C68"))
        );
        assert!(venue_quoter_v2(DexVenue::PulseX, 369).is_none());
    }

    #[test]
    fn lp_v3_venues_per_chain() {
        let on_943: Vec<_> = lp_v3_venues(943).collect();
        assert_eq!(on_943, vec![DexVenue::Wiz4rd]);
        let on_369: Vec<_> = lp_v3_venues(369).collect();
        assert!(on_369.contains(&DexVenue::NineMm));
        assert!(on_369.contains(&DexVenue::NineInch));
        assert_eq!(lp_v2_venue(369), Some(DexVenue::NineInch));
        let picker_369 = lp_v3_venue_picker(369);
        assert_eq!(
            picker_369,
            vec![DexVenue::NineInch, DexVenue::NineMm, DexVenue::Wiz4rd]
        );
        assert_eq!(lp_v3_venue_picker(943), vec![DexVenue::Wiz4rd]);
        assert!(matches!(
            lp_stack_for_chain(943),
            Some(LpStack::V3 {
                venue: DexVenue::Wiz4rd
            })
        ));
        assert!(matches!(
            lp_stack_for_chain(369),
            Some(LpStack::V3 {
                venue: DexVenue::NineInch
            })
        ));
    }

    #[test]
    fn nine_inch_v3_factory_and_npm_on_369() {
        assert_eq!(
            venue_v3_factory(DexVenue::NineInch, 369),
            Some(address!("0xCfd33C867C9F031AadfF7939Cb8086Ee5ae88c41"))
        );
        assert_eq!(
            venue_position_manager(DexVenue::NineInch, 369),
            Some(address!("0x18A532b36A9F6B10b3FEC5BF225C00A0Ec89B79E"))
        );
    }

    #[test]
    fn venue_slug_and_parse_roundtrip() {
        assert_eq!(venue_slug(DexVenue::Wiz4rd), "wiz4rd");
        assert_eq!(venue_slug(DexVenue::NineMm), "9mm");
        assert_eq!(parse_dex_venue_label("wiz4rd"), Some(DexVenue::Wiz4rd));
        assert_eq!(parse_dex_venue_label("9mm"), Some(DexVenue::NineMm));
        assert_eq!(parse_dex_venue_label("nine_mm"), Some(DexVenue::NineMm));
        assert_eq!(parse_dex_venue_label("Wiz4rd"), Some(DexVenue::Wiz4rd));
        assert!(parse_dex_venue_label("unknown").is_none());
    }

    #[test]
    fn wiz4rd_npm_on_943() {
        assert_eq!(
            venue_position_manager(DexVenue::Wiz4rd, 943),
            Some(address!("0xf1b1D004dD8bFC618F977F6ACAD127a60c566745"))
        );
    }

    #[test]
    fn write_allowed_includes_routers_and_npm() {
        let addrs: Vec<_> = write_allowed_addresses(943).collect();
        assert!(addrs.contains(&address!("0xfC656c95eCd418536844FeeaA46949bb9365BEaF")));
        assert!(addrs.contains(&address!("0xf1b1D004dD8bFC618F977F6ACAD127a60c566745")));
        assert!(addrs.contains(&address!("0x297BeFB564d3Bba2D1913613B84Fb743C259C6cf")));
        let mainnet: Vec<_> = write_allowed_addresses(369).collect();
        assert!(mainnet.contains(&address!("0xCfd33C867C9F031AadfF7939Cb8086Ee5ae88c41")));
    }

    #[test]
    fn balancer_listed_but_no_router() {
        assert!(DexVenue::Phux.unsupported_reason().is_some());
        assert!(venue_swap_router(DexVenue::Phux, DexProtocol::V2, 369).is_none());
    }
}
