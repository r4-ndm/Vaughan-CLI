//! PulseChain e*/p* token origin catalog.
//!
//! Machine-readable identity for Advisor / Sentient research tools.
//! Origin is attached **only** for known addresses — never invented.
//!
//! Verification note (2026-09-04): addresses cross-checked against Vaughan
//! [`crate::chains::evm::tokens`] (WPLS/HEX/PLSX/USDC) and public bridge
//! community e*/p* conventions (eHEX, eDAI, eUSDT, eWBTC, forks).

mod catalog;
mod entry;
mod kinds;

pub use catalog::{
    catalog_entries, entries_matching_symbol_query, is_primary_symbol, lookup, lookup_str,
};
pub use entry::{CatalogEntry, TokenOriginLabel};
pub use kinds::TokenOriginKind;
