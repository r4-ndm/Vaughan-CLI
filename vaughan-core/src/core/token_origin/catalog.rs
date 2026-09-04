//! Static PulseChain origin catalog (chain 369).
//!
//! Addresses below match Vaughan’s verified registry / bridge.pulsechain.com
//! community e*/p* conventions. Cross-checked against public scan + known
//! Vaughan `tokens.rs` entries on 2026-09-04. **Do not invent** origin for
//! addresses outside this table. Major-pair follow-ups omitted until
//! re-verified live (guidance-only slots exist on [`CatalogEntry`]).

use alloy::primitives::{address, Address};

use super::entry::CatalogEntry;
use super::kinds::TokenOriginKind;

const CHAIN_369: u64 = 369;

/// All catalogued mainnet entries.
pub fn catalog_entries() -> &'static [CatalogEntry] {
    &ENTRIES
}

/// Look up by chain + contract address (case-insensitive via Address eq).
pub fn lookup(chain_id: u64, address: Address) -> Option<&'static CatalogEntry> {
    ENTRIES
        .iter()
        .find(|e| e.chain_id == chain_id && e.address == address)
}

/// Look up from a hex string; returns `None` on parse failure or miss.
pub fn lookup_str(chain_id: u64, address: &str) -> Option<&'static CatalogEntry> {
    let addr: Address = address.trim().parse().ok()?;
    lookup(chain_id, addr)
}

/// Entries whose primary or sibling symbols match `query` (case-insensitive).
pub fn entries_matching_symbol_query(query: &str) -> Vec<&'static CatalogEntry> {
    let q = normalize_symbol(query);
    if q.is_empty() {
        return Vec::new();
    }
    ENTRIES
        .iter()
        .filter(|e| {
            e.primary_symbols
                .iter()
                .chain(e.sibling_symbols.iter())
                .any(|s| normalize_symbol(s) == q)
        })
        .collect()
}

/// True when `sym` is a primary symbol for this entry.
pub fn is_primary_symbol(entry: &CatalogEntry, sym: &str) -> bool {
    let n = normalize_symbol(sym);
    entry
        .primary_symbols
        .iter()
        .any(|s| normalize_symbol(s) == n)
}

fn normalize_symbol(s: &str) -> String {
    s.trim().to_ascii_uppercase().replace(['_', '-', ' '], "")
}

static ENTRIES: [CatalogEntry; 12] = [
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0xA1077a294dDE1B09bB078844df40758a5D0f9a27"),
        display_symbol: "WPLS",
        origin: TokenOriginKind::PulseNative,
        role: "pulse_native",
        warning: None,
        primary_symbols: &["WPLS", "W_PLS"],
        sibling_symbols: &[],
        known_major_pairs: &[],
        family: "wpls",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0x95B303987A60C71504D99Aa1b13B4DA07b0790ab"),
        display_symbol: "PLSX",
        origin: TokenOriginKind::PulseNative,
        role: "pulse_native",
        warning: None,
        primary_symbols: &["PLSX"],
        sibling_symbols: &[],
        known_major_pairs: &[],
        family: "plsx",
    },
    // pHEX — preferred HEX on PulseChain (state-fork exception).
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39"),
        display_symbol: "pHEX",
        origin: TokenOriginKind::PreferredStateFork,
        role: "preferred_state_fork",
        warning: Some("pHEX is preferred PulseChain HEX; eHEX is the bridged twin"),
        primary_symbols: &["HEX", "PHEX", "P_HEX"],
        sibling_symbols: &["EHEX", "E_HEX", "BRIDGED_HEX", "HEX_ETH"],
        known_major_pairs: &[],
        family: "hex",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0x57fde0a71132198BBeC939B98976993d8D89D225"),
        display_symbol: "eHEX",
        origin: TokenOriginKind::BridgedFromEth,
        role: "bridged",
        warning: Some("eHEX is bridged HEX — not the stakeable pHEX contract"),
        primary_symbols: &["EHEX", "E_HEX", "BRIDGED_HEX", "HEX_ETH"],
        sibling_symbols: &["HEX", "PHEX", "P_HEX"],
        known_major_pairs: &[],
        family: "hex",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0xefD766cCb38EaF1dfd701853BFCe31359239F305"),
        display_symbol: "eDAI",
        origin: TokenOriginKind::BridgedFromEth,
        role: "bridged_stable",
        warning: Some("Use bridged DAI (~$1); pDAI at the Ethereum DAI address is a state fork"),
        primary_symbols: &["DAI", "EDAI", "E_DAI", "BRIDGED_DAI"],
        sibling_symbols: &["PDAI", "FORK_DAI", "FORKED_DAI", "P_DAI"],
        known_major_pairs: &[],
        family: "dai",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0x6B175474E89094C44Da98b954EedeAC495271d0F"),
        display_symbol: "pDAI",
        origin: TokenOriginKind::StateFork,
        role: "state_fork",
        warning: Some("pDAI is a state-fork copy — not the dollar-pegged bridged DAI"),
        primary_symbols: &["PDAI", "FORK_DAI", "FORKED_DAI", "P_DAI"],
        sibling_symbols: &["DAI", "EDAI"],
        known_major_pairs: &[],
        family: "dai",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0x15D38573d2feeb82e7ad5187aB8c1D52810B1f07"),
        display_symbol: "eUSDC",
        origin: TokenOriginKind::BridgedFromEth,
        role: "bridged_stable",
        warning: None,
        primary_symbols: &["USDC", "EUSDC", "E_USDC", "BRIDGED_USDC", "USDC_ETH"],
        sibling_symbols: &[],
        known_major_pairs: &[],
        family: "usdc",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0x0Cb6F5a34ad42ec934882A05265A7d5F59b51A2f"),
        display_symbol: "eUSDT",
        origin: TokenOriginKind::BridgedFromEth,
        role: "bridged_stable",
        warning: Some("eUSDT is bridged Tether; fork USDT at 0xdAC1… is not the stable"),
        primary_symbols: &["USDT", "EUSDT", "E_USDT", "BRIDGED_USDT", "USDT_ETH"],
        sibling_symbols: &["FUSDT", "FORK_USDT", "FORKED_USDT", "P_USDT"],
        known_major_pairs: &[],
        family: "usdt",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0xdAC17F958D2ee523a2206206994597C13D831ec7"),
        display_symbol: "fUSDT",
        origin: TokenOriginKind::StateFork,
        role: "state_fork",
        warning: Some(
            "Fork USDT at the Ethereum USDT address — not bridged eUSDT; Vaughan asset scan \
             may still list this as USDT",
        ),
        primary_symbols: &["FUSDT", "FORK_USDT", "FORKED_USDT", "P_USDT"],
        sibling_symbols: &["USDT", "EUSDT"],
        known_major_pairs: &[],
        family: "usdt",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0xb17D901469B9208B17d916112988A3FeD19b5cA1"),
        display_symbol: "eWBTC",
        origin: TokenOriginKind::BridgedFromEth,
        role: "bridged",
        warning: None,
        primary_symbols: &["WBTC", "EWBTC", "E_WBTC", "BRIDGED_WBTC", "WBTC_ETH"],
        sibling_symbols: &["PWBTC", "P_WBTC", "FORK_WBTC"],
        known_major_pairs: &[],
        family: "wbtc",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"),
        display_symbol: "pWBTC",
        origin: TokenOriginKind::StateFork,
        role: "state_fork_typically_useless",
        warning: Some("pWBTC is a state-fork copy — prefer bridged eWBTC"),
        primary_symbols: &["PWBTC", "P_WBTC", "FORK_WBTC", "FORKED_WBTC"],
        sibling_symbols: &["WBTC", "EWBTC"],
        known_major_pairs: &[],
        family: "wbtc",
    },
    CatalogEntry {
        chain_id: CHAIN_369,
        address: address!("0x02DcdD04e3F455D838cd1249292C58f3B79e3C3C"),
        display_symbol: "eWETH",
        origin: TokenOriginKind::BridgedFromEth,
        role: "bridged",
        warning: Some("Bridged WETH — not the state-fork WETH at 0xC02a…"),
        primary_symbols: &["WETH", "EWETH", "E_WETH", "BRIDGED_WETH"],
        sibling_symbols: &["FWETH", "FORK_WETH", "P_WETH"],
        known_major_pairs: &[],
        family: "weth",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phex_and_eusdc_lookup() {
        let hex = lookup(369, address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39")).unwrap();
        assert_eq!(hex.display_symbol, "pHEX");
        assert_eq!(hex.origin, TokenOriginKind::PreferredStateFork);

        let usdc = lookup_str(369, "0x15D38573d2feeb82e7ad5187aB8c1D52810B1f07").unwrap();
        assert_eq!(usdc.display_symbol, "eUSDC");

        assert!(lookup(943, hex.address).is_none());
        assert!(lookup_str(369, "0x0000000000000000000000000000000000000001").is_none());
    }

    #[test]
    fn symbol_query_hex_family() {
        let hits = entries_matching_symbol_query("HEX");
        assert!(hits.iter().any(|e| e.display_symbol == "pHEX"));
        assert!(hits.iter().any(|e| e.display_symbol == "eHEX"));
        assert!(is_primary_symbol(
            hits.iter().find(|e| e.display_symbol == "pHEX").unwrap(),
            "hex"
        ));
    }

    #[test]
    fn never_invents_unknown() {
        assert!(lookup_str(369, "not-an-address").is_none());
    }
}
