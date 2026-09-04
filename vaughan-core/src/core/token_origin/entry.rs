//! Catalog entry shape and tool-facing labels.

use alloy::primitives::Address;
use serde::Serialize;

use super::kinds::TokenOriginKind;

/// One known PulseChain token for origin labeling and search guidance.
#[derive(Debug, Clone, Copy)]
pub struct CatalogEntry {
    pub chain_id: u64,
    pub address: Address,
    /// Agent-facing symbol (e.g. `eUSDC`, `pHEX`).
    pub display_symbol: &'static str,
    pub origin: TokenOriginKind,
    /// Role string for search coverage (`bridged_stable`, `preferred_state_fork`, …).
    pub role: &'static str,
    pub warning: Option<&'static str>,
    /// Symbols that select this entry as primary for a search query.
    pub primary_symbols: &'static [&'static str],
    /// Sibling symbols in the same dual-asset family.
    pub sibling_symbols: &'static [&'static str],
    /// Optional major pair addresses for `dexscreener_pair` follow-ups only.
    /// Never emit these as fabricated live search rows. Empty until re-verified.
    pub known_major_pairs: &'static [Address],
    /// Dual-asset family key (`hex`, `dai`, `usdc`, …).
    pub family: &'static str,
}

/// Compact label attached to tool results when an address is catalogued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenOriginLabel {
    pub display_symbol: String,
    pub token_origin: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    pub family: String,
}

impl CatalogEntry {
    /// Build a serializable label for MCP / sensory JSON.
    pub fn to_label(self) -> TokenOriginLabel {
        TokenOriginLabel {
            display_symbol: self.display_symbol.to_string(),
            token_origin: self.origin.as_str().to_string(),
            role: self.role.to_string(),
            identity_note: Some(format!(
                "{} is catalogued as {} ({})",
                self.display_symbol,
                self.origin.as_str(),
                self.role
            )),
            warning: self.warning.map(str::to_string),
            family: self.family.to_string(),
        }
    }
}
