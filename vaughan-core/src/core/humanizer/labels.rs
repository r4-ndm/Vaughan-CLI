//! Address → human label resolution from Vaughan's existing catalogs.
//!
//! No hosted lookup and no new data source: labels come from the on-disk
//! token-origin catalog (`token_origin`) and the DEX venue catalog
//! (`dex_catalog` / `dex_routers`). Unknown addresses render as a short hex
//! form so the card stays readable without inventing a name.

use alloy::primitives::Address;

use crate::core::dex_routers;
use crate::core::token_origin;

/// Resolve `address` on `chain_id` to a friendly label, or short hex.
///
/// Order: token catalog (e.g. `pHEX`, `WPLS`) → DEX venue/router → short hex.
pub fn label_address(chain_id: u64, address: Address) -> String {
    if let Some(entry) = token_origin::lookup(chain_id, address) {
        return entry.display_symbol.to_string();
    }
    if let Some(label) = dex_label(chain_id, address) {
        return label;
    }
    short(address)
}

/// True when `address` is a catalogued token or a known DEX write target.
pub fn is_known(chain_id: u64, address: Address) -> bool {
    token_origin::lookup(chain_id, address).is_some()
        || dex_routers::is_allowed_dex_router(chain_id, address)
}

fn dex_label(chain_id: u64, address: Address) -> Option<String> {
    dex_routers::dex_routers_labeled(chain_id)
        .into_iter()
        .find(|(a, _)| *a == address)
        .map(|(_, hint)| format!("DEX router ({hint})"))
}

/// `0x1234…cdef` short form for an unlabelled address.
fn short(address: Address) -> String {
    let s = format!("{address:#x}");
    if s.len() <= 12 {
        return s;
    }
    format!("{}…{}", &s[..6], &s[s.len() - 4..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::dex_routers::PULSEX_V2_MAINNET;
    use crate::core::hex_stake::phex_address;

    #[test]
    fn labels_known_token() {
        assert_eq!(label_address(369, phex_address()), "pHEX");
    }

    #[test]
    fn labels_dex_router() {
        let router: Address = PULSEX_V2_MAINNET.parse().unwrap();
        assert!(label_address(369, router).contains("DEX router"));
        assert!(is_known(369, router));
    }

    #[test]
    fn unknown_is_short_hex() {
        let a: Address = "0x00000000000000000000000000000000deadbeef"
            .parse()
            .unwrap();
        assert_eq!(label_address(369, a), "0x0000…beef");
        assert!(!is_known(369, a));
    }
}
