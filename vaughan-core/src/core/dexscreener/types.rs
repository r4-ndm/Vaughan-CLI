//! DexScreener DTOs and soft-fail envelopes.

use serde::Serialize;

use crate::core::token_origin::TokenOriginLabel;

/// Soft failure when upstream is unavailable or input is invalid for a soft path.
#[derive(Debug, Clone, Serialize)]
pub struct DexScreenerSoftFail {
    pub ok: bool,
    pub source: &'static str,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
}

impl DexScreenerSoftFail {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            source: "dexscreener",
            reason: reason.into(),
            status: None,
            path: None,
            chain_id: None,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_chain(mut self, chain: impl Into<String>) -> Self {
        self.chain_id = Some(chain.into());
        self
    }
}

/// One side of a pair.
#[derive(Debug, Clone, Serialize)]
pub struct DexTokenSide {
    pub address: String,
    pub name: String,
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<TokenOriginLabel>,
}

/// Minimal pair summary surfaced to agents.
#[derive(Debug, Clone, Serialize)]
pub struct DexPairSummary {
    pub chain_id: String,
    pub dex_id: String,
    pub url: String,
    pub pair_address: String,
    pub base_token: DexTokenSide,
    pub quote_token: DexTokenSide,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_usd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub liquidity_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_h24: Option<f64>,
    /// Present only on search results after spoof-aware annotate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_flags: Option<SearchFlags>,
}

/// Search-only spoof / collision annotations.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchFlags {
    pub symbol_collision: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticker_spoof_risk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub demoted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefer_address_tools: Option<bool>,
}

/// Symbol that appeared on 2+ distinct token addresses in a search set.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolCollision {
    pub symbol: String,
    pub addresses: Vec<String>,
    pub known_catalog_addresses: Vec<String>,
    pub unknown_addresses: Vec<String>,
}

/// Coverage of catalog addresses for a symbol search.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogSearchCoverage {
    pub query_matched_catalog: bool,
    pub matched_symbols: Vec<String>,
    pub present_catalog_addresses: Vec<String>,
    pub missing_catalog_entries: Vec<MissingCatalogEntry>,
    pub canonical_missing_from_upstream: bool,
    pub spoof_dominated: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MissingCatalogEntry {
    pub address: String,
    pub display_name: String,
    pub role: String,
    pub family: String,
    pub is_primary_for_query: bool,
}

/// Address-keyed follow-up (never a fabricated pair row).
#[derive(Debug, Clone, Serialize)]
pub struct AddressFollowUp {
    pub address: String,
    pub display_name: String,
    pub role: String,
    pub family: String,
    pub preferred_tool: String,
    pub reason: String,
}

/// Successful search payload.
#[derive(Debug, Clone, Serialize)]
pub struct SearchSuccess {
    pub ok: bool,
    pub source: &'static str,
    pub chain_id: String,
    pub pulsechain_only: bool,
    pub pair_count: usize,
    pub query: String,
    pub pairs: Vec<DexPairSummary>,
    pub discovery_only: bool,
    pub guidance: String,
    pub symbol_collisions: Vec<SymbolCollision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_coverage: Option<CatalogSearchCoverage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_address_followups: Option<Vec<AddressFollowUp>>,
}

pub const SEARCH_GUIDANCE: &str = "Symbol search is discovery-only and may include ticker-spoof \
contracts. Prefer dexscreener_token_pairs / dexscreener_pair / resolve_token with a verified 0x. \
Never settle identity from search alone.";
