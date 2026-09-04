//! Public DexScreener market-data client (no API key).
//!
//! Modular layout:
//! - [`chain`] — Vaughan chain id ↔ DexScreener slug
//! - [`types`] — pair summaries + soft-fail envelopes
//! - [`search`] — spoof-aware rank / catalog coverage (pure)
//! - [`client`] — HTTP + rate spacing
//!
//! Docs: `docs/dexscreener.md`. Patterns inspired by pulsechain-mcp research
//! tools; reimplemented in Rust (no TypeScript vendoring).

mod chain;
mod client;
mod search;
mod types;

pub use chain::{
    catalog_chain_id_for_dex_slug, dexscreener_chain_slug, resolve_dex_chain,
    DEFAULT_DEXSCREENER_CHAIN, DEXSCREENER_PULSECHAIN,
};
pub use client::{default_chain_slug, DexScreenerClient, DEXSCREENER_API_BASE};
pub use search::{
    attach_origin_labels, build_catalog_search_coverage, compose_search_guidance,
    rank_and_annotate_search_pairs, MAX_SEARCH_PAIRS,
};
pub use types::{
    AddressFollowUp, CatalogSearchCoverage, DexPairSummary, DexScreenerSoftFail, DexTokenSide,
    MissingCatalogEntry, SearchFlags, SearchSuccess, SymbolCollision, SEARCH_GUIDANCE,
};
