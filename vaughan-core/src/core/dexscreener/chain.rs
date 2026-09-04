//! Vaughan numeric chain id ↔ DexScreener chain slug.

/// DexScreener slug for PulseChain.
pub const DEXSCREENER_PULSECHAIN: &str = "pulsechain";

/// Default when tools omit chain / session is locked.
pub const DEFAULT_DEXSCREENER_CHAIN: &str = DEXSCREENER_PULSECHAIN;

/// Map Vaughan `chain_id` to a DexScreener chain slug.
pub fn dexscreener_chain_slug(chain_id: u64) -> Option<&'static str> {
    match chain_id {
        369 => Some(DEXSCREENER_PULSECHAIN),
        1 => Some("ethereum"),
        56 => Some("bsc"),
        137 => Some("polygon"),
        8453 => Some("base"),
        42_161 => Some("arbitrum"),
        _ => None,
    }
}

/// Resolve tool input: prefer explicit Dex slug, else map Vaughan id, else default PulseChain.
pub fn resolve_dex_chain(chain_id: Option<u64>, dex_chain: Option<&str>) -> String {
    if let Some(s) = dex_chain.map(str::trim).filter(|s| !s.is_empty()) {
        return s.to_ascii_lowercase();
    }
    if let Some(id) = chain_id {
        if let Some(slug) = dexscreener_chain_slug(id) {
            return slug.to_string();
        }
    }
    DEFAULT_DEXSCREENER_CHAIN.to_string()
}

/// Vaughan chain id for catalog lookups from a Dex slug (PulseChain only for now).
pub fn catalog_chain_id_for_dex_slug(slug: &str) -> Option<u64> {
    match slug.to_ascii_lowercase().as_str() {
        "pulsechain" => Some(369),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_pulsechain() {
        assert_eq!(resolve_dex_chain(None, None), "pulsechain");
        assert_eq!(resolve_dex_chain(Some(369), None), "pulsechain");
        assert_eq!(resolve_dex_chain(Some(1), Some("pulsechain")), "pulsechain");
    }
}
