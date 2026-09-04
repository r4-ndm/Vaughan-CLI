//! Search-only spoof-aware rank + catalog coverage (pure / unit-testable).

use std::collections::{HashMap, HashSet};

use alloy::primitives::Address;

use crate::core::token_origin::{entries_matching_symbol_query, is_primary_symbol, lookup};

use super::chain::catalog_chain_id_for_dex_slug;
use super::types::{
    AddressFollowUp, CatalogSearchCoverage, DexPairSummary, MissingCatalogEntry, SearchFlags,
    SymbolCollision, SEARCH_GUIDANCE,
};

/// Max pairs returned after ranking.
pub const MAX_SEARCH_PAIRS: usize = 20;

/// Annotate, rank, and truncate search pairs; build collision list.
pub fn rank_and_annotate_search_pairs(
    mut pairs: Vec<DexPairSummary>,
    catalog_chain_id: u64,
) -> (Vec<DexPairSummary>, Vec<SymbolCollision>) {
    let collisions = detect_symbol_collisions(&pairs, catalog_chain_id);
    let collision_syms: HashSet<String> = collisions
        .iter()
        .map(|c| c.symbol.to_ascii_uppercase())
        .collect();

    for p in &mut pairs {
        annotate_pair(p, catalog_chain_id, &collision_syms);
    }

    pairs.sort_by(|a, b| {
        let da = demoted(a);
        let db = demoted(b);
        da.cmp(&db)
            .then_with(|| risk_rank(a).cmp(&risk_rank(b)))
            .then_with(|| {
                catalog_score(b, catalog_chain_id).cmp(&catalog_score(a, catalog_chain_id))
            })
            .then_with(|| {
                liq(b)
                    .partial_cmp(&liq(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    if pairs.len() > MAX_SEARCH_PAIRS {
        pairs.truncate(MAX_SEARCH_PAIRS);
    }

    (pairs, collisions)
}

/// Catalog coverage + address follow-ups for a search query.
pub fn build_catalog_search_coverage(
    query: &str,
    pairs: &[DexPairSummary],
) -> (Option<CatalogSearchCoverage>, Vec<AddressFollowUp>) {
    let matched = entries_matching_symbol_query(query);
    if matched.is_empty() {
        return (None, Vec::new());
    }

    let present: HashSet<Address> = pairs
        .iter()
        .flat_map(|p| {
            [
                parse_side(&p.base_token.address),
                parse_side(&p.quote_token.address),
            ]
        })
        .flatten()
        .collect();

    let mut missing = Vec::new();
    let mut present_catalog = Vec::new();
    let mut matched_symbols = Vec::new();
    let mut followups = Vec::new();

    for e in &matched {
        matched_symbols.push(e.display_symbol.to_string());
        let primary = is_primary_symbol(e, query);
        if present.contains(&e.address) {
            present_catalog.push(format!("{:#x}", e.address));
        } else {
            missing.push(MissingCatalogEntry {
                address: format!("{:#x}", e.address),
                display_name: e.display_symbol.to_string(),
                role: e.role.to_string(),
                family: e.family.to_string(),
                is_primary_for_query: primary,
            });
            followups.push(AddressFollowUp {
                address: format!("{:#x}", e.address),
                display_name: e.display_symbol.to_string(),
                role: e.role.to_string(),
                family: e.family.to_string(),
                preferred_tool: "dexscreener_token_pairs".into(),
                reason:
                    "Catalogued address missing or demoted in symbol search — verify by address"
                        .into(),
            });
            for pair in e.known_major_pairs {
                followups.push(AddressFollowUp {
                    address: format!("{pair:#x}"),
                    display_name: format!("{} major pair (catalog guidance)", e.display_symbol),
                    role: e.role.to_string(),
                    family: e.family.to_string(),
                    preferred_tool: "dexscreener_pair".into(),
                    reason: "Curated major pair — not a fabricated search row".into(),
                });
            }
        }
    }

    let primary_missing = missing.iter().any(|m| m.is_primary_for_query);
    let spoof_dominated = primary_missing
        && (pairs.is_empty()
            || pairs.iter().any(|p| {
                p.search_flags
                    .as_ref()
                    .and_then(|f| f.ticker_spoof_risk.as_ref())
                    .is_some()
                    || p.search_flags.as_ref().map(|f| f.demoted == Some(true)) == Some(true)
            }));

    let note = if primary_missing && spoof_dominated {
        format!(
            "Upstream symbol search for \"{query}\" is spoof-dominated or omits the catalogued \
             primary address. Use address-keyed tools."
        )
    } else if primary_missing {
        format!(
            "Catalogued primary for \"{query}\" missing from upstream search — prefer address tools."
        )
    } else {
        "Catalogued addresses present in search results.".into()
    };

    let coverage = CatalogSearchCoverage {
        query_matched_catalog: true,
        matched_symbols,
        present_catalog_addresses: present_catalog,
        missing_catalog_entries: missing,
        canonical_missing_from_upstream: primary_missing,
        spoof_dominated,
        note,
    };

    (Some(coverage), followups)
}

pub fn compose_search_guidance(
    coverage: Option<&CatalogSearchCoverage>,
    followups: &[AddressFollowUp],
) -> String {
    let mut g = SEARCH_GUIDANCE.to_string();
    if let Some(c) = coverage {
        g.push(' ');
        g.push_str(&c.note);
    }
    if !followups.is_empty() {
        g.push_str(" See recommended_address_followups.");
    }
    g
}

/// Attach origin labels to pair sides when catalogued.
pub fn attach_origin_labels(pair: &mut DexPairSummary, catalog_chain_id: Option<u64>) {
    let Some(cid) = catalog_chain_id.or_else(|| catalog_chain_id_for_dex_slug(&pair.chain_id))
    else {
        return;
    };
    if let Some(addr) = parse_side(&pair.base_token.address) {
        if let Some(e) = lookup(cid, addr) {
            pair.base_token.origin = Some(e.to_label());
        }
    }
    if let Some(addr) = parse_side(&pair.quote_token.address) {
        if let Some(e) = lookup(cid, addr) {
            pair.quote_token.origin = Some(e.to_label());
        }
    }
}

fn annotate_pair(p: &mut DexPairSummary, catalog_chain_id: u64, collision_syms: &HashSet<String>) {
    attach_origin_labels(p, Some(catalog_chain_id));

    let base_sym = p.base_token.symbol.to_ascii_uppercase();
    let quote_sym = p.quote_token.symbol.to_ascii_uppercase();
    let base_known = parse_side(&p.base_token.address)
        .and_then(|a| lookup(catalog_chain_id, a))
        .is_some();
    let quote_known = parse_side(&p.quote_token.address)
        .and_then(|a| lookup(catalog_chain_id, a))
        .is_some();

    let mut flags = SearchFlags {
        symbol_collision: collision_syms.contains(&base_sym) || collision_syms.contains(&quote_sym),
        ticker_spoof_risk: None,
        demoted: None,
        reason: None,
        prefer_address_tools: None,
    };

    let mut worst: Option<&'static str> = None;
    for (sym, known) in [(&base_sym, base_known), (&quote_sym, quote_known)] {
        if collision_syms.contains(sym) && !known {
            worst = Some("high");
            flags.demoted = Some(true);
            flags.prefer_address_tools = Some(true);
            flags.reason = Some(format!(
                "Unknown-origin address shares ticker {sym} with another address in this result set \
                 (possible ticker spoof). Prefer address-keyed tools."
            ));
        } else if collision_syms.contains(sym) && known && worst.is_none() {
            worst = Some("low");
        }
    }
    flags.ticker_spoof_risk = worst.map(str::to_string);
    if flags.symbol_collision || flags.demoted == Some(true) || flags.ticker_spoof_risk.is_some() {
        p.search_flags = Some(flags);
    }
}

fn detect_symbol_collisions(
    pairs: &[DexPairSummary],
    catalog_chain_id: u64,
) -> Vec<SymbolCollision> {
    let mut by_sym: HashMap<String, HashSet<String>> = HashMap::new();
    for p in pairs {
        for (sym, addr) in [
            (&p.base_token.symbol, &p.base_token.address),
            (&p.quote_token.symbol, &p.quote_token.address),
        ] {
            let s = sym.to_ascii_uppercase();
            if s.is_empty() {
                continue;
            }
            by_sym
                .entry(s)
                .or_default()
                .insert(addr.to_ascii_lowercase());
        }
    }

    let mut out = Vec::new();
    for (symbol, addrs) in by_sym {
        if addrs.len() < 2 {
            continue;
        }
        let mut known = Vec::new();
        let mut unknown = Vec::new();
        for a in &addrs {
            if let Ok(addr) = a.parse::<Address>() {
                if lookup(catalog_chain_id, addr).is_some() {
                    known.push(a.clone());
                } else {
                    unknown.push(a.clone());
                }
            } else {
                unknown.push(a.clone());
            }
        }
        out.push(SymbolCollision {
            symbol,
            addresses: addrs.into_iter().collect(),
            known_catalog_addresses: known,
            unknown_addresses: unknown,
        });
    }
    out
}

fn demoted(p: &DexPairSummary) -> u8 {
    if p.search_flags.as_ref().and_then(|f| f.demoted) == Some(true) {
        1
    } else {
        0
    }
}

fn risk_rank(p: &DexPairSummary) -> u8 {
    match p
        .search_flags
        .as_ref()
        .and_then(|f| f.ticker_spoof_risk.as_deref())
    {
        Some("high") => 3,
        Some("medium") => 2,
        Some("low") => 1,
        _ => 0,
    }
}

fn catalog_score(p: &DexPairSummary, catalog_chain_id: u64) -> u8 {
    let mut s = 0u8;
    if parse_side(&p.base_token.address)
        .and_then(|a| lookup(catalog_chain_id, a))
        .is_some()
    {
        s += 1;
    }
    if parse_side(&p.quote_token.address)
        .and_then(|a| lookup(catalog_chain_id, a))
        .is_some()
    {
        s += 1;
    }
    s
}

fn liq(p: &DexPairSummary) -> f64 {
    p.liquidity_usd.unwrap_or(0.0)
}

fn parse_side(s: &str) -> Option<Address> {
    s.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::dexscreener::types::DexTokenSide;

    fn side(addr: &str, sym: &str) -> DexTokenSide {
        DexTokenSide {
            address: addr.into(),
            name: sym.into(),
            symbol: sym.into(),
            origin: None,
        }
    }

    fn pair(base: (&str, &str), quote: (&str, &str), liq: f64) -> DexPairSummary {
        DexPairSummary {
            chain_id: "pulsechain".into(),
            dex_id: "pulsex".into(),
            url: String::new(),
            pair_address: "0x1111111111111111111111111111111111111111".into(),
            base_token: side(base.0, base.1),
            quote_token: side(quote.0, quote.1),
            price_usd: None,
            liquidity_usd: Some(liq),
            volume_h24: None,
            search_flags: None,
        }
    }

    #[test]
    fn demotes_spoof_hex_ahead_of_catalog() {
        let phex = "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";
        let spoof = "0x1111111111111111111111111111111111111111";
        let wpls = "0xA1077a294dDE1B09bB078844df40758a5D0f9a27";
        let pairs = vec![
            pair((spoof, "HEX"), (wpls, "WPLS"), 9_999_999.0),
            pair((phex, "HEX"), (wpls, "WPLS"), 100.0),
        ];
        let (ranked, collisions) = rank_and_annotate_search_pairs(pairs, 369);
        assert!(!collisions.is_empty());
        assert_eq!(
            ranked[0].base_token.address.to_ascii_lowercase(),
            phex.to_ascii_lowercase()
        );
        assert_eq!(
            ranked[1].search_flags.as_ref().and_then(|f| f.demoted),
            Some(true)
        );
    }

    #[test]
    fn coverage_when_canonical_missing() {
        let pairs = vec![pair(
            ("0x1111111111111111111111111111111111111111", "HEX"),
            ("0xA1077a294dDE1B09bB078844df40758a5D0f9a27", "WPLS"),
            1.0,
        )];
        let (ranked, _) = rank_and_annotate_search_pairs(pairs, 369);
        let (cov, follow) = build_catalog_search_coverage("HEX", &ranked);
        let cov = cov.expect("coverage");
        assert!(cov.canonical_missing_from_upstream || !cov.missing_catalog_entries.is_empty());
        assert!(!follow.is_empty());
    }
}
